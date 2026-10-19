//! SONORA desktop shell (Tauri 2).
//!
//! Deliberately thin: it opens the analysis core (`crates/sonora-core`),
//! exposes one IPC command that forwards JSON calls to it, relays core events
//! to the webview, and registers the plugins it needs: single-instance (one
//! process per library), dialog (native file pickers), opener ("Reveal in
//! Finder / File Explorer"), updater and process (updates from GitHub
//! releases, then a restart). All application logic lives in the core, where
//! it is tested without a webview.

use serde::Serialize;
use serde_json::json;
use sonora_core::{App as Core, AppConfig};
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tauri::http::{header, Request, Response, StatusCode};
use tauri::{AppHandle, Emitter, Manager, RunEvent};

/// Webview event that carries every core event as `{ name, payload }`.
const EVENT_CHANNEL: &str = "sonora://event";

/// URI scheme the web view plays audio from: `sonora://localhost/<id>` on
/// macOS and `http://sonora.localhost/<id>` on Windows. It serves only the
/// files the core prepared for playback, under an id handed out with them,
/// so the interface cannot read anything else on disk (Tauri's general asset
/// protocol would also expose folders picked in dialogs or dropped on the
/// window).
const AUDIO_SCHEME: &str = "sonora";
/// Most bytes sent for one request; the audio element asks for the rest.
const AUDIO_CHUNK: u64 = 2 * 1024 * 1024;
/// Prepared files that stay playable (the most recent ones).
const AUDIO_KEEP: usize = 16;

/// Files the core prepared for playback: (id, path, MIME type), newest last.
#[derive(Default)]
struct AudioFiles {
    next: AtomicU64,
    files: Mutex<Vec<(String, PathBuf, String)>>,
}

impl AudioFiles {
    fn add(&self, path: PathBuf, mime: String) -> String {
        let id = format!("t{}", self.next.fetch_add(1, Ordering::Relaxed) + 1);
        let mut files = self.files.lock().unwrap_or_else(|e| e.into_inner());
        files.push((id.clone(), path, mime));
        let excess = files.len().saturating_sub(AUDIO_KEEP);
        files.drain(..excess);
        id
    }

    fn get(&self, id: &str) -> Option<(PathBuf, String)> {
        let files = self.files.lock().unwrap_or_else(|e| e.into_inner());
        files.iter().find(|(i, _, _)| i == id).map(|(_, p, m)| (p.clone(), m.clone()))
    }
}

/// `bytes=a-b`, `bytes=a-` or `bytes=-n` → first and last byte, or `Err` when
/// it lies outside the file. `None` when there is no usable range.
fn parse_range(value: &str, len: u64) -> Option<Result<(u64, u64), ()>> {
    let spec = value.trim().strip_prefix("bytes=")?;
    let (a, b) = spec.split(',').next()?.trim().split_once('-')?;
    let (a, b) = (a.trim(), b.trim());
    let range = if a.is_empty() {
        let n: u64 = b.parse().ok()?;
        if n == 0 {
            return Some(Err(()));
        }
        (len.saturating_sub(n), len.checked_sub(1)?)
    } else {
        let start: u64 = a.parse().ok()?;
        let end = if b.is_empty() { len.saturating_sub(1) } else { b.parse::<u64>().ok()?.min(len.saturating_sub(1)) };
        (start, end)
    };
    if len == 0 || range.0 >= len || range.0 > range.1 {
        return Some(Err(()));
    }
    Some(Ok(range))
}

fn plain(status: StatusCode) -> Response<Vec<u8>> {
    Response::builder().status(status).body(Vec::new()).unwrap_or_default()
}

/// Answers one request of the audio element (range requests for seeking).
fn serve_audio(files: &AudioFiles, request: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let id = request.uri().path().trim_start_matches('/');
    let Some((path, mime)) = files.get(id) else { return plain(StatusCode::NOT_FOUND) };
    let Ok(mut file) = std::fs::File::open(&path) else { return plain(StatusCode::NOT_FOUND) };
    let Ok(len) = file.metadata().map(|m| m.len()) else { return plain(StatusCode::INTERNAL_SERVER_ERROR) };
    let range = request.headers().get(header::RANGE).and_then(|v| v.to_str().ok()).and_then(|v| parse_range(v, len));
    let (status, start, end) = match range {
        Some(Ok((start, end))) => (StatusCode::PARTIAL_CONTENT, start, end.min(start + AUDIO_CHUNK - 1)),
        Some(Err(())) => {
            return Response::builder()
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(header::CONTENT_RANGE, format!("bytes */{len}"))
                .body(Vec::new())
                .unwrap_or_default()
        }
        None if len == 0 || request.method() == tauri::http::Method::HEAD => {
            return Response::builder()
                .header(header::CONTENT_TYPE, mime)
                .header(header::ACCEPT_RANGES, "bytes")
                .header(header::CONTENT_LENGTH, len.to_string())
                .body(Vec::new())
                .unwrap_or_default()
        }
        None => (StatusCode::OK, 0, len - 1),
    };
    let mut body = vec![0u8; (end - start + 1) as usize];
    if file.seek(SeekFrom::Start(start)).and_then(|_| file.read_exact(&mut body)).is_err() {
        return plain(StatusCode::INTERNAL_SERVER_ERROR);
    }
    let mut response = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, mime)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, body.len().to_string());
    if status == StatusCode::PARTIAL_CONTENT {
        response = response.header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{len}"));
    }
    response.body(body).unwrap_or_default()
}

/// sonora-core declares the SQLite C API without linking a library (feature
/// `external-sqlite`); libsqlite3-sys supplies it, compiled from source, so
/// every platform runs the same SQLite. This reference makes the linker keep
/// that SQLite whatever order it visits the two crates in.
#[used]
static LINK_SQLITE: unsafe extern "C" fn() -> *const std::ffi::c_char = libsqlite3_sys::sqlite3_libversion;

/// Error returned to the frontend; mirrors `sonora_core::ApiError`.
#[derive(Debug, Serialize)]
struct ApiErrorDto {
    code: String,
    message: String,
}

impl ApiErrorDto {
    fn new(code: &str, message: impl Into<String>) -> Self {
        ApiErrorDto { code: code.to_string(), message: message.into() }
    }

    fn internal() -> Self {
        ApiErrorDto::new("internal_error", "SONORA ran into an internal error while handling this request. Details are in the log.")
    }
}

/// Payload of [`EVENT_CHANNEL`].
#[derive(Clone, Serialize)]
struct CoreEvent<'a> {
    name: &'a str,
    payload: serde_json::Value,
}

/// Managed state: the opened core, or why it could not be opened (reported
/// to the interface on every call instead of crashing the app).
struct Shell {
    core: Result<Arc<Core>, String>,
    subscription: Mutex<Option<u64>>,
}

/// Writes a structured line through the core's logger: JSON on stderr and,
/// once the core has opened, in `<app data>/logs/sonora.log`. The core already
/// logs its own failures and slow calls; the shell only logs what happens
/// around it.
fn log(level: &str, message: &str, fields: serde_json::Value) {
    let fields = sonora_json::parse(&fields.to_string()).unwrap_or(sonora_json::Value::Null);
    sonora_core::logging::log_fields(level, "desktop", message, fields);
}

/// Forwards one API call to the core on a blocking thread, so neither the
/// main thread nor the IPC handler waits for database or analysis work. The
/// core's JSON is passed to the webview as is, exactly as the browser
/// development bridge delivers it.
#[tauri::command]
async fn sonora_call(
    app: AppHandle,
    state: tauri::State<'_, Shell>,
    method: String,
    params: serde_json::Value,
) -> Result<tauri::ipc::Response, ApiErrorDto> {
    let core = match &state.core {
        Ok(core) => Arc::clone(core),
        Err(reason) => return Err(ApiErrorDto::new("core_unavailable", reason.clone())),
    };
    let params_json = if params.is_null() { "{}".to_string() } else { params.to_string() };
    let name = method.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || core.call(&method, &params_json)).await.map_err(|e| {
        log("error", "core call panicked", json!({ "method": &name, "error": e.to_string() }));
        ApiErrorDto::internal()
    })?;
    match outcome {
        Ok(text) if name == "playback.prepare" => Ok(tauri::ipc::Response::new(register_playback(&app, text))),
        Ok(text) => Ok(tauri::ipc::Response::new(text)),
        Err(e) => Err(ApiErrorDto { code: e.code, message: e.message }),
    }
}

/// Makes the file the core prepared for playback reachable through
/// [`AUDIO_SCHEME`], and adds its id (`stream`) to the core's answer.
fn register_playback(app: &AppHandle, core_result: String) -> String {
    let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&core_result) else { return core_result };
    let path = v.get("path").and_then(|p| p.as_str()).map(PathBuf::from);
    let mime = v.get("mime").and_then(|m| m.as_str()).unwrap_or("application/octet-stream").to_string();
    let Some(path) = path else { return core_result };
    let id = app.state::<AudioFiles>().add(path, mime);
    v["stream"] = json!(id);
    v.to_string()
}

/// Stops the core's background work (scans, exports, lookups) before an
/// update is installed: on Windows the installer ends this process, on macOS
/// the app restarts into the new version. The library database is not
/// touched by updates, so settings, edits and results are kept.
#[tauri::command]
async fn prepare_update(app: AppHandle) -> Result<(), ApiErrorDto> {
    log("info", "stopping the core for an update", json!({}));
    tauri::async_runtime::spawn_blocking(move || shutdown(&app)).await.map_err(|_| ApiErrorDto::internal())
}

/// In debug builds, prefer an FFmpeg fetched into `src-tauri/binaries/` by
/// `scripts/fetch-ffmpeg.mjs`; release builds bundle it next to the
/// executable instead. An explicit `SONORA_FFMPEG` always wins.
fn configure_dev_ffmpeg() {
    #[cfg(debug_assertions)]
    {
        if std::env::var_os("SONORA_FFMPEG").is_some() {
            return;
        }
        let candidate = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("binaries")
            .join(format!("ffmpeg-{}{}", env!("SONORA_TARGET_TRIPLE"), std::env::consts::EXE_SUFFIX));
        if candidate.is_file() {
            // Runs first thing in `run`, before any other thread exists.
            std::env::set_var("SONORA_FFMPEG", &candidate);
        }
    }
}

fn open_core(handle: &AppHandle) -> Shell {
    let failed = |reason: String| {
        log("error", "library could not be opened", json!({ "reason": &reason }));
        Shell { core: Err(reason), subscription: Mutex::new(None) }
    };
    // The local (non-roaming) data folder: on Windows %LOCALAPPDATA%, so a
    // roaming profile never carries the database around. On macOS and Linux
    // this is the usual application data folder.
    let data_dir = match handle.path().app_local_data_dir() {
        Ok(dir) => dir,
        Err(e) => return failed(format!("The application data folder could not be determined: {e}")),
    };
    // Where a bundled FFmpeg can live: the resource folder and next to the
    // executable (Tauri places `externalBin` sidecars there).
    let mut tool_dirs = Vec::new();
    if let Ok(dir) = handle.path().resource_dir() {
        tool_dirs.push(dir);
    }
    if let Some(dir) = tauri::utils::platform::current_exe().ok().and_then(|exe| exe.parent().map(|p| p.to_path_buf())) {
        if !tool_dirs.contains(&dir) {
            tool_dirs.push(dir);
        }
    }
    let config = AppConfig { data_dir: data_dir.clone(), tool_dirs, app_version: handle.package_info().version.to_string() };
    match Core::open(config) {
        Ok(core) => {
            let emitter = handle.clone();
            let id = core.subscribe(Box::new(move |name: &str, payload: &str| {
                let payload = serde_json::from_str(payload).unwrap_or(serde_json::Value::Null);
                if let Err(e) = emitter.emit(EVENT_CHANNEL, CoreEvent { name, payload }) {
                    log("warn", "event could not be delivered", json!({ "name": name, "error": e.to_string() }));
                }
            }));
            Shell { core: Ok(core), subscription: Mutex::new(Some(id)) }
        }
        Err(e) => failed(format!("The SONORA library in {} could not be opened: {e}", data_dir.display())),
    }
}

fn shutdown(handle: &AppHandle) {
    let Some(shell) = handle.try_state::<Shell>() else { return };
    if let Ok(core) = &shell.core {
        if let Some(id) = shell.subscription.lock().ok().and_then(|mut s| s.take()) {
            core.unsubscribe(id);
        }
        core.shutdown();
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    configure_dev_ffmpeg();
    let app = tauri::Builder::default()
        // Registered first: a second launch hands over to the running app
        // (whose window comes to the front) and exits, so two processes never
        // open the same library and analyse the same tracks.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_process::init())
        .manage(AudioFiles::default())
        .register_asynchronous_uri_scheme_protocol(AUDIO_SCHEME, |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            // Reading a chunk is quick, but never on the thread that asked.
            std::thread::spawn(move || responder.respond(serve_audio(&app.state::<AudioFiles>(), &request)));
        })
        .invoke_handler(tauri::generate_handler![sonora_call, prepare_update])
        .setup(|app| {
            // Updates are signed; the public key and the GitHub endpoint are in
            // tauri.conf.json (plugins > updater).
            #[cfg(desktop)]
            app.handle().plugin(tauri_plugin_updater::Builder::new().build())?;
            let shell = open_core(app.handle());
            app.manage(shell);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building the SONORA application");
    app.run(|handle, event| {
        if let RunEvent::Exit = event {
            shutdown(handle);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::parse_range;

    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=0-", 100), Some(Ok((0, 99))));
        assert_eq!(parse_range("bytes=10-19", 100), Some(Ok((10, 19))));
        assert_eq!(parse_range("bytes=90-200", 100), Some(Ok((90, 99))));
        assert_eq!(parse_range("bytes=-10", 100), Some(Ok((90, 99))));
        assert_eq!(parse_range("bytes=100-", 100), Some(Err(())));
        assert_eq!(parse_range("bytes=5-2", 100), Some(Err(())));
        assert_eq!(parse_range("items=0-1", 100), None);
        assert_eq!(parse_range("bytes=x-1", 100), None);
    }
}
