//! SONORA desktop shell (Tauri 2).
//!
//! Deliberately thin: it opens the analysis core (`crates/sonora-core`),
//! exposes one IPC command that forwards JSON calls to it, relays core events
//! to the webview, and registers the plugins it needs: single-instance (one
//! process per library), dialog (native file pickers) and opener ("Reveal in
//! Finder / File Explorer"). All application logic lives in the core, where
//! it is tested without a webview.

use serde::Serialize;
use serde_json::json;
use sonora_core::{App as Core, AppConfig};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, RunEvent};

/// Webview event that carries every core event as `{ name, payload }`.
const EVENT_CHANNEL: &str = "sonora://event";

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
        Ok(text) => {
            if name == "playback.prepare" {
                allow_playback(&app, &text);
            }
            Ok(tauri::ipc::Response::new(text))
        }
        Err(e) => Err(ApiErrorDto { code: e.code, message: e.message }),
    }
}

/// Lets the web view read the one file the core prepared for playback
/// through the asset protocol, whose scope is otherwise empty: the
/// interface can never read arbitrary files from disk.
fn allow_playback(app: &AppHandle, core_result: &str) {
    let path = serde_json::from_str::<serde_json::Value>(core_result).ok().and_then(|v| v.get("path").and_then(|p| p.as_str()).map(str::to_string));
    if let Some(path) = path {
        if let Err(e) = app.asset_protocol_scope().allow_file(&path) {
            log("warn", "could not allow playback of a file", json!({ "error": e.to_string() }));
        }
    }
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
        .invoke_handler(tauri::generate_handler![sonora_call])
        .setup(|app| {
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
