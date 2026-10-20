//! Listening to Soulseek files before downloading them, against a stand-in
//! slskd that "receives" files into its folder for partial downloads a
//! piece at a time (resuming from what is there, as slskd does), moves a
//! finished one into the batch's folder, and records what SONORA asked.

use sonora_core::net::{Http, NetError, Request, Response};
use sonora_core::preview::{incomplete_path, ReadError};
use sonora_core::{App, AppConfig};
use sonora_json::{obj, Value};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const KEY: &str = "preview-key-0123456789";
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

// ------------------------------------------------------------- FLAC files

fn crc8(data: &[u8]) -> u8 {
    let mut c = 0u8;
    for &b in data {
        c ^= b;
        for _ in 0..8 {
            c = if c & 0x80 != 0 { (c << 1) ^ 0x07 } else { c << 1 };
        }
    }
    c
}

fn crc16(data: &[u8]) -> u16 {
    let mut c = 0u16;
    for &b in data {
        c ^= (b as u16) << 8;
        for _ in 0..8 {
            c = if c & 0x8000 != 0 { (c << 1) ^ 0x8005 } else { c << 1 };
        }
    }
    c
}

fn utf8_number(n: u64) -> Vec<u8> {
    if n < 0x80 {
        vec![n as u8]
    } else if n < 0x800 {
        vec![0xC0 | (n >> 6) as u8, 0x80 | (n & 0x3F) as u8]
    } else {
        vec![0xE0 | (n >> 12) as u8, 0x80 | ((n >> 6) & 0x3F) as u8, 0x80 | (n & 0x3F) as u8]
    }
}

/// The sample of channel `c` at frame `i` (a different tone in each channel).
fn sample(i: u64, c: usize) -> i16 {
    let f = if c == 0 { 440.0 } else { 660.0 };
    ((i as f64 * f * 2.0 * std::f64::consts::PI / 44_100.0).sin() * 12_000.0) as i16
}

/// A stereo 16-bit 44.1 kHz FLAC of `seconds`, in verbatim frames.
/// `declare_length`: STREAMINFO says how long it is.
fn flac(seconds: u64, declare_length: bool) -> Vec<u8> {
    let total = seconds * 44_100;
    let block = 4096u64;
    let mut out = b"fLaC".to_vec();
    out.extend_from_slice(&[0x80, 0, 0, 34]);
    let mut si = Vec::new();
    si.extend_from_slice(&(block as u16).to_be_bytes());
    si.extend_from_slice(&(block as u16).to_be_bytes());
    si.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
    let t = if declare_length { total } else { 0 };
    // 20 bits rate, 3 bits channels-1, 5 bits bps-1, 36 bits total samples.
    let packed: u64 = (44_100u64 << 44) | (1u64 << 41) | (15u64 << 36) | t;
    si.extend_from_slice(&packed.to_be_bytes());
    si.extend_from_slice(&[0u8; 16]);
    out.extend_from_slice(&si);
    let mut frame_no = 0u64;
    let mut i = 0u64;
    while i < total {
        let n = block.min(total - i);
        let mut f = vec![0xFF, 0xF8, 0x79, 0x18];
        f.extend(utf8_number(frame_no));
        f.extend_from_slice(&((n - 1) as u16).to_be_bytes());
        let c8 = crc8(&f);
        f.push(c8);
        for c in 0..2 {
            f.push(0x02);
            for k in 0..n {
                f.extend_from_slice(&sample(i + k, c).to_be_bytes());
            }
        }
        let c16 = crc16(&f);
        f.extend_from_slice(&c16.to_be_bytes());
        out.extend_from_slice(&f);
        i += n;
        frame_no += 1;
    }
    out
}

// --------------------------------------------------------- the stand-in

#[derive(Clone, Debug)]
struct T {
    id: String,
    username: String,
    filename: String,
    size: i64,
    state: String,
    batch: Option<String>,
    destination: Option<String>,
    removed: bool,
    /// Where it resumed from.
    offset: u64,
}

#[derive(Default)]
struct World {
    transfers: Vec<T>,
    calls: Vec<String>,
    /// The other users' files.
    files: HashMap<String, Vec<u8>>,
    /// Bytes written per tick (every 20 ms).
    rate: usize,
    /// Fail transfers once they reach this many bytes (then clear it).
    fail_at: Option<u64>,
    reject: bool,
    next_id: u64,
}

struct Fake {
    world: Mutex<World>,
    root: PathBuf,
    stops: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

impl Fake {
    fn incomplete(&self) -> PathBuf {
        self.root.join("incomplete")
    }
    fn downloads(&self) -> PathBuf {
        self.root.join("downloads")
    }

    fn transfer_json(t: &T) -> Value {
        obj! {
            "id" => t.id.clone(), "username" => t.username.clone(), "filename" => t.filename.clone(), "size" => t.size,
            "state" => t.state.clone(), "bytesTransferred" => 0, "batchId" => t.batch.clone(),
            "placeInQueue" => if t.state.starts_with("Queued") { Value::from(1) } else { Value::Null },
        }
    }

    /// Starts "receiving" a file: from what is already in the partial file on.
    fn start(self: &Arc<Self>, username: &str, filename: &str, size: i64, batch: Option<String>, destination: Option<String>) -> String {
        let id = {
            let mut w = self.world.lock().unwrap();
            w.next_id += 1;
            format!("00000000-0000-4000-8000-{:012x}", w.next_id)
        };
        let partial = incomplete_path(&self.incomplete(), username, filename, cfg!(windows)).unwrap();
        let offset = std::fs::metadata(&partial).map(|m| m.len()).unwrap_or(0);
        let t = T { id: id.clone(), username: username.into(), filename: filename.into(), size, state: "Queued, Remotely".into(), batch, destination, removed: false, offset };
        self.world.lock().unwrap().transfers.push(t);
        let stop = Arc::new(AtomicBool::new(false));
        self.stops.lock().unwrap().insert(id.clone(), stop.clone());
        let me = Arc::clone(self);
        let (id2, user, name) = (id.clone(), username.to_string(), filename.to_string());
        std::thread::spawn(move || me.receive(&id2, &user, &name, &partial, offset, &stop));
        id
    }

    fn set_state(&self, id: &str, state: &str) {
        if let Some(t) = self.world.lock().unwrap().transfers.iter_mut().find(|t| t.id == id) {
            t.state = state.into();
        }
    }

    fn receive(&self, id: &str, username: &str, filename: &str, partial: &Path, offset: u64, stop: &AtomicBool) {
        std::thread::sleep(Duration::from_millis(60));
        if self.world.lock().unwrap().reject {
            self.set_state(id, "Completed, Rejected");
            return;
        }
        let data = self.world.lock().unwrap().files.get(filename).cloned().expect("a file the stand-in shares");
        std::fs::create_dir_all(partial.parent().unwrap()).unwrap();
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(partial).unwrap();
        self.set_state(id, "InProgress");
        let mut pos = offset as usize;
        while pos < data.len() {
            if stop.load(Ordering::Relaxed) {
                self.set_state(id, "Completed, Cancelled");
                return;
            }
            let (rate, fail_at) = {
                let w = self.world.lock().unwrap();
                (w.rate, w.fail_at)
            };
            if fail_at.is_some_and(|n| pos as u64 >= n) {
                self.world.lock().unwrap().fail_at = None;
                self.set_state(id, "Completed, Errored");
                return;
            }
            let n = rate.min(data.len() - pos);
            f.write_all(&data[pos..pos + n]).unwrap();
            f.flush().unwrap();
            pos += n;
            std::thread::sleep(Duration::from_millis(20));
        }
        drop(f);
        // Finished: into the batch's folder (or the download folder).
        let dest = self.world.lock().unwrap().transfers.iter().find(|t| t.id == id).and_then(|t| t.destination.clone());
        let dir = match dest {
            Some(d) => self.downloads().join(d),
            None => self.downloads().join("from-soulseek"),
        };
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::rename(partial, dir.join(partial.file_name().unwrap())).unwrap();
        let _ = username;
        self.set_state(id, "Completed, Succeeded");
    }
}

struct FakeHttp(Arc<Fake>);

impl Http for FakeHttp {
    fn send(&self, req: &Request) -> Result<Response, NetError> {
        let this = &self.0;
        let path = req.url.strip_prefix("http://localhost:5030/api/v0/").unwrap().to_string();
        this.world.lock().unwrap().calls.push(format!("{} {path}", req.method));
        if req.headers.iter().find(|(k, _)| k == "X-API-Key").map(|(_, v)| v.as_str()) != Some(KEY) {
            return Ok(Response { status: 401, body: String::new() });
        }
        let ok = |v: Value| Ok(Response { status: 200, body: v.to_string_compact() });
        let (route, query) = path.split_once('?').unwrap_or((path.as_str(), ""));
        let parts: Vec<String> = route.split('/').map(|p| p.replace("%20", " ")).collect();
        match (req.method, parts.iter().map(String::as_str).collect::<Vec<_>>().as_slice()) {
            ("GET", ["options"]) => ok(obj! {
                "directories" => obj! { "downloads" => this.downloads().to_string_lossy().into_owned(), "incomplete" => this.incomplete().to_string_lossy().into_owned() },
                "transfers" => obj! { "download" => obj! { "retry" => obj! { "partial" => "resume" } } },
            }),
            ("POST", ["transfers", "downloads", "batches"]) => {
                let b = sonora_json::parse(req.body.as_deref().unwrap_or("")).unwrap();
                let file = &b.get("files").unwrap().as_array().unwrap()[0];
                let filename = file.get("filename").and_then(Value::as_str).unwrap();
                let busy = this.world.lock().unwrap().transfers.iter().any(|t| t.filename == filename && !t.removed && !t.state.starts_with("Completed"));
                if busy {
                    return ok(obj! { "batch" => obj! {}, "failures" => Value::Array(vec![obj! { "filename" => filename, "message" => "already in progress" }]) });
                }
                let id = this.start(
                    b.get("username").and_then(Value::as_str).unwrap(),
                    filename,
                    file.get("size").and_then(Value::as_i64).unwrap(),
                    b.get("id").and_then(Value::as_str).map(str::to_string),
                    b.get("options").and_then(|o| o.get("destination")).and_then(Value::as_str).map(str::to_string),
                );
                let t = this.world.lock().unwrap().transfers.iter().find(|t| t.id == id).cloned().unwrap();
                Ok(Response { status: 201, body: obj! { "batch" => obj! { "transfers" => Value::Array(vec![Fake::transfer_json(&t)]) }, "failures" => Value::Array(vec![]) }.to_string_compact() })
            }
            ("POST", ["transfers", "downloads", user]) => {
                let files = sonora_json::parse(req.body.as_deref().unwrap_or("")).unwrap();
                let mut failed = Vec::new();
                for f in files.as_array().unwrap() {
                    let filename = f.get("filename").and_then(Value::as_str).unwrap();
                    let busy = this.world.lock().unwrap().transfers.iter().any(|t| t.filename == filename && !t.removed && !t.state.starts_with("Completed"));
                    if busy {
                        failed.push(Value::String(filename.into()));
                    } else {
                        this.start(user, filename, f.get("size").and_then(Value::as_i64).unwrap(), None, None);
                    }
                }
                Ok(Response { status: 201, body: obj! { "enqueued" => Value::Array(vec![]), "failed" => Value::Array(failed) }.to_string_compact() })
            }
            ("GET", ["transfers", "downloads"]) => {
                let w = this.world.lock().unwrap();
                let list: Vec<Value> = w.transfers.iter().filter(|t| !t.removed).map(|t| obj! { "username" => t.username.clone(), "directories" => Value::Array(vec![obj! { "directory" => "x", "files" => Value::Array(vec![Fake::transfer_json(t)]) }]) }).collect();
                ok(Value::Array(list))
            }
            ("GET", ["transfers", "downloads", user]) => {
                let w = this.world.lock().unwrap();
                let files: Vec<Value> = w.transfers.iter().filter(|t| !t.removed && t.username == *user).map(Fake::transfer_json).collect();
                if files.is_empty() {
                    return Ok(Response { status: 404, body: String::new() });
                }
                ok(obj! { "username" => *user, "directories" => Value::Array(vec![obj! { "directory" => "x", "files" => Value::Array(files) }]) })
            }
            ("GET", ["transfers", "downloads", _user, id]) => {
                let w = this.world.lock().unwrap();
                match w.transfers.iter().find(|t| t.id == *id && !t.removed) {
                    Some(t) => ok(Fake::transfer_json(t)),
                    None => Ok(Response { status: 404, body: String::new() }),
                }
            }
            ("DELETE", ["transfers", "downloads", _user, id]) => {
                if let Some(s) = this.stops.lock().unwrap().get(*id) {
                    s.store(true, Ordering::Relaxed);
                }
                let mut w = this.world.lock().unwrap();
                match w.transfers.iter_mut().find(|t| t.id == *id) {
                    Some(t) => {
                        if query.contains("remove=true") {
                            t.removed = true;
                        }
                        Ok(Response { status: 204, body: String::new() })
                    }
                    None => Ok(Response { status: 404, body: String::new() }),
                }
            }
            _ => Ok(Response { status: 404, body: String::new() }),
        }
    }
}

// ------------------------------------------------------------- helpers

struct Env {
    root: PathBuf,
    app: Arc<App>,
    fake: Arc<Fake>,
}

fn env(name: &str) -> Env {
    std::env::set_var("SONORA_SOURCE_POLL_SECONDS", "0");
    std::env::set_var("SONORA_SECRET_STORE", "file");
    let root = std::env::temp_dir().join(format!("sonora-preview-it-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let app = App::open(AppConfig { data_dir: root.join("data"), tool_dirs: vec![], app_version: "test".into() }).unwrap();
    let fake = Arc::new(Fake { world: Mutex::new(World { rate: 64 * 1024, ..World::default() }), root: root.join("slskd"), stops: Mutex::new(HashMap::new()) });
    std::fs::create_dir_all(fake.downloads()).unwrap();
    app.slskd.set_http(Arc::new(FakeHttp(fake.clone())));
    call(&app, "sources.accept", &obj! {});
    call(&app, "slskd.configure", &obj! { "apiKey" => KEY });
    call(&app, "sources.configure", &obj! { "folder" => fake.downloads().to_string_lossy().into_owned() });
    call(&app, "settings.update", &obj! { "values" => obj! { "previewAhead" => 20 } });
    Env { root, app, fake }
}

fn call(app: &Arc<App>, method: &str, params: &Value) -> Value {
    let out = app.call(method, &params.to_string_compact()).unwrap_or_else(|e| panic!("{method}: {e}"));
    sonora_json::parse(&out).unwrap()
}

fn call_err(app: &Arc<App>, method: &str, params: &Value) -> (String, String) {
    let e = app.call(method, &params.to_string_compact()).expect_err(method);
    (e.code, e.message)
}

fn status(app: &Arc<App>, id: i64) -> Value {
    call(app, "preview.status", &obj! { "id" => id })
}

fn wait_for(what: &str, secs: u64, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(30));
    }
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

const USER: &str = "crate-digger";
const FILE: &str = "@@abcde\\Music\\Night Drive\\01 Lights.flac";

fn share(e: &Env, name: &str, data: Vec<u8>) -> i64 {
    let n = data.len() as i64;
    e.fake.world.lock().unwrap().files.insert(name.to_string(), data);
    n
}

fn partial(e: &Env, name: &str) -> PathBuf {
    incomplete_path(&e.fake.incomplete(), USER, name, cfg!(windows)).unwrap()
}

// --------------------------------------------------------------- tests

#[test]
fn plays_before_the_file_has_arrived_reads_ahead_only_so_far_and_leaves_nothing() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let e = env("flow");
    let app = &e.app;
    let size = share(&e, FILE, flac(60, true));

    let p = call(app, "preview.start", &obj! { "username" => USER, "filename" => FILE, "size" => size, "length" => 60 });
    let id = p.get("id").and_then(Value::as_i64).unwrap();
    assert_eq!(s(&p, "state"), "connecting");

    // Playable after a moment, long before the file is all there.
    wait_for("the preview to be ready", 20, || status(app, id).get("ready").and_then(Value::as_bool) == Some(true));
    let st = status(app, id);
    assert!(st.get("received").and_then(Value::as_i64).unwrap() < size, "it started before the whole file arrived: {st:?}");
    assert_eq!(st.get("totalSeconds").and_then(Value::as_f64), Some(60.0));
    let chunk = app.previews.read(id as u64, 0, 64 * 1024, Duration::from_secs(5)).unwrap();
    assert_eq!(&chunk.bytes[0..4], b"RIFF");
    assert_eq!(chunk.total, 44 + 60 * 44_100 * 4);
    // The audio is the file's.
    for i in [0u64, 1000, 5000] {
        let at = 44 + i as usize * 4;
        let l = i16::from_le_bytes([chunk.bytes[at], chunk.bytes[at + 1]]);
        let r = i16::from_le_bytes([chunk.bytes[at + 2], chunk.bytes[at + 3]]);
        assert_eq!((l, r), (sample(i, 0), sample(i, 1)), "frame {i}");
    }

    // A preview is not a download: it is asked for as a batch of its own,
    // landing in the preview folder, and kept off the downloads list.
    let calls = e.fake.world.lock().unwrap().calls.clone();
    assert!(calls.iter().any(|c| c == "POST transfers/downloads/batches"), "{calls:?}");
    assert!(!calls.iter().any(|c| c == &format!("POST transfers/downloads/{USER}")), "no download was asked for: {calls:?}");
    let dest = e.fake.world.lock().unwrap().transfers[0].destination.clone().unwrap();
    assert_eq!(dest, format!(".sonora-preview/{id}"));
    let list = call(app, "slskd.downloads", &obj! {});
    assert_eq!(list.get("downloads").and_then(Value::as_array).map(Vec::len), Some(0), "{list:?}");

    // 20 seconds ahead of what plays is enough: the transfer stops there,
    // with the file far from complete.
    wait_for("the transfer to pause", 20, || s(&status(app, id), "transfer") == "paused");
    let st = status(app, id);
    let received = st.get("received").and_then(Value::as_i64).unwrap();
    assert!(received < size * 3 / 4, "read ahead only so far: {received} of {size}");
    assert!(e.fake.world.lock().unwrap().calls.iter().any(|c| c.starts_with("DELETE transfers/downloads/")));

    // Playing on: it asks again, and the stand-in resumes from what is there.
    call(app, "preview.position", &obj! { "id" => id, "seconds" => 25.0 });
    wait_for("the transfer to resume", 20, || e.fake.world.lock().unwrap().transfers.len() >= 2);
    let second = e.fake.world.lock().unwrap().transfers[1].clone();
    assert!(second.offset > 0, "resumed, not started over");
    wait_for("more audio", 20, || status(app, id).get("decodedSeconds").and_then(Value::as_f64).unwrap_or(0.0) > 30.0);

    // A read beyond what arrived waits for it.
    call(app, "preview.position", &obj! { "id" => id, "seconds" => 59.0 });
    let late = 44 + 55 * 44_100 * 4;
    let c = app.previews.read(id as u64, late, 4096, Duration::from_secs(30)).unwrap();
    assert_eq!(c.start, late);
    assert_eq!(c.bytes.len(), 4096);
    // Past the end: nothing.
    assert!(matches!(app.previews.read(id as u64, chunk.total, 10, Duration::from_millis(10)), Err(ReadError::Unsatisfiable(_))));

    // Stopping leaves nothing: no transfer in slskd, no partial file, no cache.
    wait_for("the whole file", 20, || s(&status(app, id), "transfer") == "complete");
    let preview_dir = e.fake.downloads().join(".sonora-preview").join(id.to_string());
    assert!(preview_dir.is_dir(), "it arrived whole, in its own folder");
    // The folder watcher does not take it.
    let check = call(app, "sources.check", &obj! {});
    assert_eq!(check.get("found").and_then(|f| f.get("settling")).and_then(Value::as_i64), Some(0));
    call(app, "preview.stop", &obj! { "id" => id });
    wait_for("the cleanup", 20, || !preview_dir.exists() && !e.root.join("data").join("cache").join("previews").join(format!("preview-{id}.pcm")).exists());
    assert!(!partial(&e, FILE).exists());
    assert!(!e.fake.downloads().join(".sonora-preview").exists());
    assert!(!e.root.join("data").join("cache").join("previews").join("previews.json").exists());
    wait_for("every transfer of the preview to leave slskd's list", 10, || e.fake.world.lock().unwrap().transfers.iter().all(|t| t.removed));
    let tracks = call(app, "library.query", &obj! { "limit" => 10 });
    assert_eq!(tracks.get("total").and_then(Value::as_i64), Some(0), "nothing joined the library");
    assert_eq!(call_err(app, "preview.status", &obj! { "id" => id }).0, "not_found");
    let _ = std::fs::remove_dir_all(&e.root);
}

#[test]
fn stopping_part_way_cancels_the_transfer_and_removes_the_partial_file() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let e = env("stop");
    let app = &e.app;
    e.fake.world.lock().unwrap().rate = 16 * 1024;
    let size = share(&e, FILE, flac(60, true));
    let id = call(app, "preview.start", &obj! { "username" => USER, "filename" => FILE, "size" => size }).get("id").and_then(Value::as_i64).unwrap();
    wait_for("some audio", 20, || status(app, id).get("ready").and_then(Value::as_bool) == Some(true));
    assert!(partial(&e, FILE).exists());
    call(app, "preview.stop", &obj! { "id" => id });
    wait_for("the transfer to be taken off slskd's list", 20, || e.fake.world.lock().unwrap().transfers.iter().all(|t| t.removed));
    // The partial file goes, and its folders too, up to slskd's folder for partial downloads.
    wait_for("the partial file and its folders to go", 20, || !partial(&e, FILE).exists() && !e.fake.incomplete().join(USER).exists());
    assert!(e.fake.incomplete().exists());
    let _ = std::fs::remove_dir_all(&e.root);
}

#[test]
fn download_while_listening_resumes_from_what_the_preview_fetched() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let e = env("handover");
    let app = &e.app;
    e.fake.world.lock().unwrap().rate = 16 * 1024;
    let size = share(&e, FILE, flac(30, true));
    let id = call(app, "preview.start", &obj! { "username" => USER, "filename" => FILE, "size" => size }).get("id").and_then(Value::as_i64).unwrap();
    wait_for("some audio", 20, || status(app, id).get("ready").and_then(Value::as_bool) == Some(true));
    let r = call(app, "slskd.download", &obj! { "username" => USER, "files" => Value::Array(vec![obj! { "filename" => FILE, "size" => size }]) });
    assert_eq!(r.get("queued").and_then(Value::as_i64), Some(1), "{r:?}");
    let ts = e.fake.world.lock().unwrap().transfers.clone();
    assert_eq!(ts.len(), 2);
    assert!(ts[0].removed, "the preview's transfer stopped");
    assert!(ts[1].batch.is_none() && ts[1].offset > 0, "the download resumed from what the preview fetched: {:?}", ts[1]);
    let st = status(app, id);
    assert_eq!(st.get("handedOver").and_then(Value::as_bool), Some(true));
    // The download shows in the downloads list; the preview keeps playing along.
    wait_for("the download to be listed", 10, || call(app, "slskd.downloads", &obj! {}).get("downloads").and_then(Value::as_array).map(Vec::len) == Some(1));
    wait_for("the rest of the audio", 30, || status(app, id).get("decodedSeconds").and_then(Value::as_f64).unwrap_or(0.0) >= 29.9);
    // Stopping the preview leaves the download alone.
    call(app, "preview.stop", &obj! { "id" => id });
    std::thread::sleep(Duration::from_millis(500));
    assert!(!e.fake.world.lock().unwrap().transfers[1].removed);
    assert!(e.fake.downloads().join("from-soulseek").join("01 Lights.flac").exists());
    let _ = std::fs::remove_dir_all(&e.root);
}

#[test]
fn what_cannot_be_previewed_says_why_and_a_lost_connection_is_resumed() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let e = env("problems");
    let app = &e.app;
    let (code, msg) = call_err(app, "preview.start", &obj! { "username" => USER, "filename" => "x\\a.mp3", "size" => 100 });
    assert_eq!(code, "preview_unavailable");
    assert!(msg.contains("FLAC, WAV and AIFF"), "{msg}");
    assert_eq!(call_err(app, "preview.start", &obj! { "username" => USER, "filename" => "x\\a.m4a", "size" => 100 }).0, "preview_unavailable");

    // The other user's client drops the connection once: SONORA asks again and
    // the transfer goes on from where it stopped.
    let size = share(&e, FILE, flac(20, true));
    e.fake.world.lock().unwrap().fail_at = Some(200_000);
    let id = call(app, "preview.start", &obj! { "username" => USER, "filename" => FILE, "size" => size }).get("id").and_then(Value::as_i64).unwrap();
    wait_for("the whole file after a failure", 30, || s(&status(app, id), "transfer") == "complete");
    let ts = e.fake.world.lock().unwrap().transfers.clone();
    assert_eq!(ts.len(), 2, "{ts:?}");
    assert!(ts[0].state.contains("Errored") && ts[1].offset >= 200_000);
    call(app, "preview.stop", &obj! { "id" => id });

    // The other user says no: the preview fails and says why.
    e.fake.world.lock().unwrap().reject = true;
    let other = "@@abcde\\Music\\Night Drive\\02 Glow.flac";
    let size = share(&e, other, flac(5, true));
    let id = call(app, "preview.start", &obj! { "username" => USER, "filename" => other, "size" => size }).get("id").and_then(Value::as_i64).unwrap();
    wait_for("the refusal", 10, || s(&status(app, id), "state") == "failed");
    assert!(status(app, id).get("error").and_then(Value::as_str).unwrap().contains("turned the request down"));
    call(app, "preview.stop", &obj! { "id" => id });
    e.fake.world.lock().unwrap().reject = false;

    // A FLAC that does not say how long it is: with a length from the
    // search results it plays; without one it cannot.
    let unknown = "@@abcde\\Music\\Night Drive\\03 Unknown.flac";
    let size = share(&e, unknown, flac(5, false));
    let id = call(app, "preview.start", &obj! { "username" => USER, "filename" => unknown, "size" => size }).get("id").and_then(Value::as_i64).unwrap();
    wait_for("it to give up", 10, || s(&status(app, id), "state") == "unavailable");
    assert!(status(app, id).get("error").and_then(Value::as_str).unwrap().contains("how long"));
    call(app, "preview.stop", &obj! { "id" => id });
    let id = call(app, "preview.start", &obj! { "username" => USER, "filename" => unknown, "size" => size, "length" => 5 }).get("id").and_then(Value::as_i64).unwrap();
    wait_for("it to play", 10, || status(app, id).get("ready").and_then(Value::as_bool) == Some(true));
    assert_eq!(status(app, id).get("totalSeconds").and_then(Value::as_f64), Some(5.0));
    call(app, "preview.stop", &obj! { "id" => id });
    let _ = std::fs::remove_dir_all(&e.root);
}

#[test]
fn a_wav_previews_too_and_a_new_preview_stops_the_last_one() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let e = env("wav");
    let app = &e.app;
    let wav_name = "@@abcde\\Music\\Night Drive\\04 Tape.wav";
    let tmp = e.root.join("tape.wav");
    let samples: Vec<f32> = (0..44_100u64 * 4 * 2).map(|i| sample(i / 2, (i % 2) as usize) as f32 / 32768.0).collect();
    sonora_engine::decode::wav::write_wav(&tmp, 44_100, 2, 16, false, &samples).unwrap();
    let size = share(&e, wav_name, std::fs::read(&tmp).unwrap());
    let flac_size = share(&e, FILE, flac(20, true));
    let first = call(app, "preview.start", &obj! { "username" => USER, "filename" => FILE, "size" => flac_size }).get("id").and_then(Value::as_i64).unwrap();
    wait_for("the first", 20, || status(app, first).get("ready").and_then(Value::as_bool) == Some(true));
    let id = call(app, "preview.start", &obj! { "username" => USER, "filename" => wav_name, "size" => size }).get("id").and_then(Value::as_i64).unwrap();
    assert_eq!(call_err(app, "preview.status", &obj! { "id" => first }).0, "not_found", "one preview at a time");
    wait_for("the WAV", 20, || status(app, id).get("ready").and_then(Value::as_bool) == Some(true));
    let c = app.previews.read(id as u64, 44 + 4 * 100, 8, Duration::from_secs(5)).unwrap();
    assert_eq!(i16::from_le_bytes([c.bytes[0], c.bytes[1]]), sample(100, 0));
    call(app, "preview.stop", &obj! { "id" => id });
    wait_for("both cleaned up", 20, || !partial(&e, FILE).exists() && !partial(&e, wav_name).exists());
    let _ = std::fs::remove_dir_all(&e.root);
}

#[test]
fn what_previews_left_when_sonora_quit_is_removed_at_the_next_start() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let e = env("recover");
    let app = &e.app;
    let size = share(&e, FILE, flac(10, true));
    // A transfer and a partial file a preview left behind.
    let tid = e.fake.start(USER, FILE, size, Some("b".into()), Some(".sonora-preview/1".into()));
    wait_for("some bytes", 10, || partial(&e, FILE).exists());
    let cache = e.root.join("data").join("cache").join("previews");
    std::fs::create_dir_all(&cache).unwrap();
    std::fs::write(cache.join("preview-1.pcm"), b"x").unwrap();
    let ledger = Value::Array(vec![obj! {
        "id" => 1, "username" => USER, "filename" => FILE, "transfers" => Value::Array(vec![Value::String(tid.clone())]),
        "partial" => partial(&e, FILE).to_string_lossy().into_owned(), "root" => e.fake.incomplete().to_string_lossy().into_owned(),
        "preexisting" => false, "dir" => e.fake.downloads().join(".sonora-preview").join("1").to_string_lossy().into_owned(), "handedOver" => false,
    }]);
    std::fs::write(cache.join("previews.json"), ledger.to_string_compact()).unwrap();
    sonora_core::preview::recover(app);
    assert!(e.fake.world.lock().unwrap().transfers.iter().all(|t| t.removed));
    assert!(!partial(&e, FILE).exists());
    assert!(!cache.join("preview-1.pcm").exists());
    assert!(!cache.join("previews.json").exists());
    // And a partial file that was there before a preview (a download's) stays.
    std::fs::create_dir_all(partial(&e, FILE).parent().unwrap()).unwrap();
    std::fs::write(partial(&e, FILE), b"someone's download").unwrap();
    let ledger = Value::Array(vec![obj! {
        "id" => 2, "username" => USER, "filename" => FILE, "transfers" => Value::Array(vec![]),
        "partial" => partial(&e, FILE).to_string_lossy().into_owned(), "root" => e.fake.incomplete().to_string_lossy().into_owned(),
        "preexisting" => true, "handedOver" => false,
    }]);
    std::fs::write(cache.join("previews.json"), ledger.to_string_compact()).unwrap();
    sonora_core::preview::recover(app);
    assert!(partial(&e, FILE).exists());
    let _ = std::fs::remove_dir_all(&e.root);
}
