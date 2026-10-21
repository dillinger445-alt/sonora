//! Searching Soulseek and downloading through slskd
//! (<https://github.com/slskd/slskd>), a full Soulseek client the person
//! installs and runs with their own account and their own sharing settings.
//!
//! SONORA never connects to Soulseek. It sends slskd instructions through
//! slskd's web API, on this computer unless the person sets another address:
//! search for these words, download these files, cancel that download. slskd
//! does everything on the network. Finished downloads land in slskd's
//! download folder, which the download-folder watcher (`sources.rs`) adds to
//! the library and checks.
//!
//! What SONORA sends to slskd: the search words and the files chosen, with
//! slskd's API key (kept in the system's keychain, see `secrets.rs`). slskd
//! passes the search words, with the person's Soulseek user name, to the
//! network, where other users see them. Requests go through the system's curl
//! (`net.rs`). Nothing about other users is stored by SONORA.
//!
//! Formats, bit depths and sample rates in results are what other users'
//! clients report and are not verified; SONORA's analysis checks each file
//! once it arrives.

use crate::app::App;
use crate::db::sqlite::{Connection, SqlValue};
use crate::db::StoreError;
use crate::library::{self, as_params};
use crate::net::{self, Http, NetError, Request, Response};
use sonora_json::{obj, ToJson, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

/// Where slskd answers unless the person sets another address.
pub const DEFAULT_URL: &str = "http://localhost:5030";
/// The API key's name in the system's store.
pub const KEY_NAME: &str = "slskd-api-key";
const TIMEOUT: Duration = Duration::from_secs(20);

/// Formats that keep all of the audio, by file extension. ALAC in `.m4a`
/// files cannot be told from AAC by name, so `.m4a` is not counted.
pub const LOSSLESS: &[&str] = &["flac", "wav", "wave", "bwf", "aif", "aiff", "aifc", "alac", "wv", "ape"];

/// Most folders and files a results list holds (best first).
const MAX_FOLDERS: usize = 150;
const MAX_FILES: usize = 3000;
/// Most downloads listed.
const MAX_TRANSFERS: usize = 500;

#[derive(Clone, Debug, PartialEq)]
pub struct SlskdError {
    pub code: &'static str,
    pub message: String,
}

impl SlskdError {
    pub fn new(code: &'static str, message: impl Into<String>) -> SlskdError {
        SlskdError { code, message: message.into() }
    }
}

impl From<StoreError> for SlskdError {
    fn from(e: StoreError) -> Self {
        SlskdError::new("storage_error", e.to_string())
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// The connection to slskd.
pub struct Slskd {
    http: Mutex<Arc<dyn Http>>,
    /// The API key once read from the system's store (`Some(None)`: none saved).
    key: Mutex<Option<Option<String>>>,
}

impl Default for Slskd {
    fn default() -> Self {
        Slskd { http: Mutex::new(Arc::new(net::Curl::default())), key: Mutex::new(None) }
    }
}

/// Checks and tidies the address of slskd's web interface.
pub fn normalize_url(raw: &str) -> Result<String, String> {
    let url = raw.trim().trim_end_matches('/');
    let rest = url.strip_prefix("http://").or_else(|| url.strip_prefix("https://")).ok_or("the address must start with http:// or https://")?;
    let host = rest.split('/').next().unwrap_or("");
    if host.is_empty() || host.starts_with(':') {
        return Err("the address needs a computer name, such as localhost".into());
    }
    if rest.chars().any(|c| c.is_control() || c.is_whitespace() || matches!(c, '@' | '?' | '#' | '\\' | '"' | '<' | '>')) {
        return Err("the address may hold only a computer name, a port and a path".into());
    }
    Ok(url.to_string())
}

/// Whether the address is this computer.
pub fn is_local(url: &str) -> bool {
    let rest = url.split("://").nth(1).unwrap_or("");
    let host_port = rest.split('/').next().unwrap_or("");
    let host = if let Some(h) = host_port.strip_prefix('[') { h.split(']').next().unwrap_or("") } else { host_port.split(':').next().unwrap_or("") };
    let host = host.to_ascii_lowercase();
    host == "localhost" || host == "::1" || host.starts_with("127.")
}

/// Folders where slskd keeps finished downloads unless told otherwise
/// (its application folder, which depends on the system), those that exist.
pub fn default_folders() -> Vec<String> {
    let mut c: Vec<PathBuf> = Vec::new();
    if cfg!(windows) {
        if let Some(d) = std::env::var_os("LOCALAPPDATA") {
            c.push(PathBuf::from(d).join("slskd").join("downloads"));
        }
    } else if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        if cfg!(target_os = "macos") {
            c.push(home.join("Library").join("Application Support").join("slskd").join("downloads"));
        }
        if let Some(x) = std::env::var_os("XDG_DATA_HOME").filter(|x| !x.is_empty()) {
            c.push(PathBuf::from(x).join("slskd").join("downloads"));
        }
        c.push(home.join(".local").join("share").join("slskd").join("downloads"));
    }
    let mut seen = HashSet::new();
    c.into_iter().filter(|p| p.is_dir() && seen.insert(p.clone())).map(|p| p.to_string_lossy().into_owned()).collect()
}

/// A slskd identifier (a GUID), checked before it goes into an address.
fn is_guid(s: &str) -> bool {
    s.len() == 36 && s.bytes().enumerate().all(|(i, b)| if [8, 13, 18, 23].contains(&i) { b == b'-' } else { b.is_ascii_hexdigit() })
}

/// Checks an API key: slskd accepts 16 to 255 characters.
pub fn check_key(key: &str) -> Result<(), String> {
    let n = key.chars().count();
    if !(16..=255).contains(&n) {
        return Err("slskd API keys are 16 to 255 characters long".into());
    }
    if key.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("an API key cannot contain spaces or line breaks".into());
    }
    Ok(())
}

/// What slskd wrote in an error answer (a JSON string or plain text).
fn error_text(body: &str) -> String {
    let t = body.trim();
    let t = sonora_json::parse(t).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_else(|| t.to_string());
    let t: String = t.chars().filter(|c| !c.is_control()).take(240).collect();
    t.trim().to_string()
}

/// Milliseconds since 1970 from slskd's timestamps
/// ("2026-10-20T10:01:02.1234567Z", with an offset, or without one for UTC).
pub fn parse_time(s: &str) -> Option<i64> {
    let s = s.trim();
    let b = s.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || !(b[10] == b'T' || b[10] == b' ') || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let num = |a: usize, z: usize| s.get(a..z).and_then(|x| x.parse::<i64>().ok());
    let (y, mo, d, h, mi, se) = (num(0, 4)?, num(5, 7)?, num(8, 10)?, num(11, 13)?, num(14, 16)?, num(17, 19)?);
    let mut rest = &s[19..];
    let mut ms = 0i64;
    if let Some(r) = rest.strip_prefix('.') {
        let n = r.bytes().take_while(u8::is_ascii_digit).count();
        let digits = &r[..n];
        ms = format!("{:0<3}", &digits[..n.min(3)]).parse().unwrap_or(0);
        rest = &r[n..];
    }
    let offset_min = match rest {
        "" | "Z" | "z" => 0,
        o if o.len() == 6 && (o.starts_with('+') || o.starts_with('-')) && o.as_bytes()[3] == b':' => {
            let sign = if o.starts_with('-') { -1 } else { 1 };
            sign * (o.get(1..3)?.parse::<i64>().ok()? * 60 + o.get(4..6)?.parse::<i64>().ok()?)
        }
        _ => return None,
    };
    // Days since 1970-01-01 (Howard Hinnant's algorithm).
    let yy = if mo <= 2 { y - 1 } else { y };
    let era = if yy >= 0 { yy } else { yy - 399 } / 400;
    let yoe = yy - era * 400;
    let doy = (153 * ((mo + 9) % 12) + 2) / 5 + d - 1;
    let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
    Some((days * 86_400 + h * 3_600 + mi * 60 + se - offset_min * 60) * 1_000 + ms)
}

/// A remote path's folder and file name (Soulseek paths use either slash).
fn split_remote(path: &str) -> (&str, &str) {
    match path.rfind(['\\', '/']) {
        Some(i) => (&path[..i], &path[i + 1..]),
        None => ("", path),
    }
}

fn last_segment(dir: &str) -> &str {
    dir.rsplit(['\\', '/']).find(|s| !s.is_empty()).unwrap_or(dir)
}

fn extension(name: &str) -> String {
    match name.rfind('.') {
        Some(i) if i + 1 < name.len() => name[i + 1..].to_ascii_lowercase(),
        _ => String::new(),
    }
}

/// Letters and digits only, lower case: names compared this way survive the
/// small changes clients make when saving (replaced characters, a suffix).
fn plain(name: &str) -> String {
    name.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

fn stem(name: &str) -> &str {
    match name.rfind('.') {
        Some(i) if i > 0 => &name[..i],
        _ => name,
    }
}

/// A file name without its extension.
pub fn stem_of(name: &str) -> &str {
    stem(name)
}

/// One file in search results.
#[derive(Clone, Debug, PartialEq)]
pub struct FoundFile {
    /// The full path on the other user's computer, as slskd needs it.
    pub filename: String,
    pub name: String,
    pub ext: String,
    pub size: i64,
    pub bit_depth: Option<i64>,
    pub sample_rate: Option<i64>,
    pub bit_rate: Option<i64>,
    pub length: Option<i64>,
    pub lossless: bool,
}

/// One folder of one user's results.
#[derive(Clone, Debug, PartialEq)]
pub struct FoundFolder {
    pub username: String,
    pub directory: String,
    pub name: String,
    pub files: Vec<FoundFile>,
    pub free_slot: bool,
    pub queue: i64,
    /// Upload speed the user's client reports, in bytes a second.
    pub speed: i64,
    /// Files in this folder left out by the format filter.
    pub hidden: usize,
}

fn found_file(f: &Value) -> Option<FoundFile> {
    let filename = f.get("filename").and_then(Value::as_str)?.to_string();
    let name = split_remote(&filename).1.to_string();
    let mut ext = extension(&name);
    if ext.is_empty() {
        ext = f.get("extension").and_then(Value::as_str).unwrap_or("").trim_start_matches('.').to_ascii_lowercase();
    }
    let int = |k: &str| f.get(k).and_then(Value::as_i64).filter(|v| *v > 0);
    Some(FoundFile {
        lossless: LOSSLESS.contains(&ext.as_str()),
        size: f.get("size").and_then(Value::as_i64).unwrap_or(0).max(0),
        bit_depth: int("bitDepth"),
        sample_rate: int("sampleRate"),
        bit_rate: int("bitRate"),
        length: int("length"),
        filename,
        name,
        ext,
    })
}

fn is_audio(f: &FoundFile) -> bool {
    crate::index::AUDIO_EXTENSIONS.contains(&f.ext.as_str())
}

/// Groups slskd's search responses into folders, best first: a free upload
/// slot, then a short queue, then speed, then more matching files. With
/// `lossless_only`, other formats are left out (and counted as hidden).
pub fn group(responses: &[Value], lossless_only: bool) -> Vec<FoundFolder> {
    let mut folders: Vec<FoundFolder> = Vec::new();
    let mut index: HashMap<(String, String), usize> = HashMap::new();
    for r in responses {
        let Some(username) = r.get("username").and_then(Value::as_str) else { continue };
        let free_slot = r.get("hasFreeUploadSlot").and_then(Value::as_bool).unwrap_or(false);
        let queue = r.get("queueLength").and_then(Value::as_i64).unwrap_or(0).max(0);
        let speed = r.get("uploadSpeed").and_then(Value::as_i64).unwrap_or(0).max(0);
        // Locked files are shared only with the user's friends: never listed.
        // Nor are covers, cue sheets and the like: only audio joins the library.
        for f in r.get("files").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]) {
            let Some(file) = found_file(f).filter(is_audio) else { continue };
            let directory = split_remote(&file.filename).0.to_string();
            let key = (username.to_string(), directory.clone());
            let i = *index.entry(key).or_insert_with(|| {
                folders.push(FoundFolder {
                    username: username.to_string(),
                    name: last_segment(&directory).to_string(),
                    directory,
                    files: Vec::new(),
                    free_slot,
                    queue,
                    speed,
                    hidden: 0,
                });
                folders.len() - 1
            });
            let folder = &mut folders[i];
            if lossless_only && !file.lossless {
                folder.hidden += 1;
            } else {
                folder.files.push(file);
            }
        }
    }
    folders.retain(|f| !f.files.is_empty());
    for f in &mut folders {
        f.files.sort_by_key(|a| a.filename.to_lowercase());
    }
    folders.sort_by(|a, b| {
        b.free_slot
            .cmp(&a.free_slot)
            .then(a.queue.cmp(&b.queue))
            .then(b.speed.cmp(&a.speed))
            .then(b.files.len().cmp(&a.files.len()))
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then(a.username.cmp(&b.username))
    });
    folders
}

/// What a transfer's state (slskd's flags, such as "Completed, Succeeded")
/// means: requested, queued (in the other user's queue), waiting (in
/// slskd's own queue), starting, downloading, done, failed or cancelled.
pub fn transfer_state(flags: &str) -> &'static str {
    let has = |f: &str| flags.split(',').any(|p| p.trim().eq_ignore_ascii_case(f));
    if has("Succeeded") {
        "done"
    } else if has("Cancelled") {
        "cancelled"
    } else if has("Completed") || has("Errored") || has("TimedOut") || has("Rejected") || has("Aborted") {
        "failed"
    } else if has("InProgress") {
        "downloading"
    } else if has("Initializing") {
        "starting"
    } else if has("Queued") {
        if has("Locally") {
            "waiting"
        } else {
            "queued"
        }
    } else {
        "requested"
    }
}

/// Why a transfer failed, in words.
fn failure(flags: &str, exception: Option<&str>) -> String {
    let has = |f: &str| flags.split(',').any(|p| p.trim().eq_ignore_ascii_case(f));
    let detail = exception.map(str::trim).filter(|e| !e.is_empty()).map(|e| e.chars().take(200).collect::<String>());
    let base = if has("Rejected") {
        "The other user turned the request down"
    } else if has("TimedOut") {
        "The other user did not answer in time"
    } else if has("Aborted") {
        "The transfer was stopped"
    } else {
        "The download failed"
    };
    match detail {
        Some(d) if !base.contains(&d) => format!("{base}: {d}"),
        _ => base.to_string(),
    }
}

/// The error for an answer from slskd that is not a success.
fn status_error(r: &Response) -> SlskdError {
    match r.status {
        401 => SlskdError::new(
            "slskd_unauthorized",
            "slskd did not accept the API key. Check that the key in slskd.yml matches the one saved here, that its cidr setting includes this computer, and that slskd was restarted after the change.",
        ),
        403 => SlskdError::new(
            "slskd_forbidden",
            "slskd refused the request. A slskd running as a relay agent does not search or download: enter the address of the main slskd. Otherwise give the key the readwrite role in slskd.yml.",
        ),
        404 => SlskdError::new("not_found", "slskd no longer has that. It may have been removed in slskd."),
        409 => SlskdError::new("slskd_offline", "slskd is not connected to Soulseek right now. Check its connection in slskd's web interface."),
        429 => SlskdError::new("slskd_busy", "slskd is busy with another request. Try again in a moment."),
        s => {
            let t = error_text(&r.body);
            SlskdError::new("slskd_failed", if t.is_empty() { format!("slskd reported an error ({s}).") } else { format!("slskd reported an error ({s}): {t}") })
        }
    }
}

impl Slskd {
    /// Replaces the network access (tests).
    pub fn set_http(&self, h: Arc<dyn Http>) {
        *lock(&self.http) = h;
    }

    /// The API key, read from the system's store once.
    pub fn key(&self, app: &App) -> Result<Option<String>, SlskdError> {
        let mut k = lock(&self.key);
        if k.is_none() {
            let stored = app.secrets.get(KEY_NAME).map_err(|e| SlskdError::new("secret_store", format!("The API key could not be read: {e}")))?;
            *k = Some(stored);
        }
        Ok(k.clone().flatten())
    }

    /// Saves (or with `None` removes) the API key.
    pub fn set_key(&self, app: &App, key: Option<&str>) -> Result<(), SlskdError> {
        let saved = match key {
            Some(k) => app.secrets.set(KEY_NAME, k),
            None => app.secrets.delete(KEY_NAME),
        };
        saved.map_err(|e| SlskdError::new("secret_store", format!("The API key could not be saved: {e}")))?;
        *lock(&self.key) = Some(key.map(str::to_string));
        Ok(())
    }

    /// What the interface shows about the connection (nothing is asked of slskd).
    pub fn status_json(&self, app: &App) -> Result<Value, SlskdError> {
        let url = app.settings().slskd_url;
        let key = self.key(app);
        Ok(obj! {
            "url" => url.clone(),
            "local" => is_local(&url),
            "keySaved" => key.as_ref().map(|k| k.is_some()).unwrap_or(false),
            "keyError" => key.err().map(|e| Value::String(e.message)).unwrap_or(Value::Null),
            "keyStore" => app.secrets.label(),
            "defaultFolders" => default_folders(),
        })
    }

    /// Sends one request to slskd's API; any HTTP status comes back.
    fn send(&self, app: &App, method: &'static str, path: &str, body: Option<Value>) -> Result<Response, SlskdError> {
        let key = self.key(app)?.ok_or_else(|| SlskdError::new("slskd_not_set_up", "Add slskd's API key on the Soulseek page first."))?;
        let base = app.settings().slskd_url;
        let req = Request {
            method,
            url: format!("{base}/api/v0/{path}"),
            headers: vec![("X-API-Key".into(), key), ("Accept".into(), "application/json".into())],
            body: body.map(|b| b.to_string_compact()),
            timeout: TIMEOUT,
        };
        let http = Arc::clone(&lock(&self.http));
        http.send(&req).map_err(|e| match e {
            NetError::NoCurl(m) => SlskdError::new("slskd_failed", m),
            NetError::Unreachable(_) => SlskdError::new("slskd_unreachable", format!("SONORA could not reach slskd at {base}. Is slskd running?")),
            NetError::Certificate(_) => SlskdError::new(
                "slskd_certificate",
                "SONORA could not check slskd's https certificate. slskd makes a new certificate each time it starts, which no system trusts: use http:// for slskd on this computer, or give slskd a certificate your system trusts.",
            ),
            NetError::TimedOut => SlskdError::new("slskd_unreachable", format!("slskd at {base} did not answer in time.")),
            NetError::Failed(m) => SlskdError::new("slskd_failed", format!("The request to slskd failed ({m}).")),
        })
    }

    /// Sends a request and turns error answers into errors.
    fn call(&self, app: &App, method: &'static str, path: &str, body: Option<Value>) -> Result<Response, SlskdError> {
        let r = self.send_patiently(app, method, path, body)?;
        if (200..300).contains(&r.status) || r.status == 304 {
            Ok(r)
        } else {
            Err(status_error(&r))
        }
    }

    /// Sends a request, waiting a moment and asking again while slskd says
    /// it is busy (it takes one search and two download requests at a time).
    fn send_patiently(&self, app: &App, method: &'static str, path: &str, body: Option<Value>) -> Result<Response, SlskdError> {
        let mut attempt = 0;
        loop {
            let r = self.send(app, method, path, body.clone())?;
            if r.status != 429 || attempt >= 4 {
                return Ok(r);
            }
            attempt += 1;
            std::thread::sleep(Duration::from_millis(500 * attempt));
        }
    }

    fn json(r: &Response) -> Result<Value, SlskdError> {
        sonora_json::parse(&r.body).map_err(|_| SlskdError::new("slskd_failed", "slskd sent an answer SONORA could not read. Is the address that of slskd?"))
    }

    /// Asks slskd how it is doing: version, Soulseek account and connection, shares.
    pub fn test(&self, app: &App) -> Result<Value, SlskdError> {
        let v = Self::json(&self.call(app, "GET", "application", None)?)?;
        let server = v.get("server");
        let state = server.and_then(|s| s.get("state")).and_then(Value::as_str).unwrap_or("");
        let flag = |k: &str, word: &str| server.and_then(|s| s.get(k)).and_then(Value::as_bool).unwrap_or_else(|| state.split(',').any(|p| p.trim() == word));
        let version = v.get("version").and_then(|x| x.get("current").or_else(|| x.get("full"))).and_then(Value::as_str).unwrap_or("");
        let shares = v.get("shares");
        let count = |k: &str| shares.and_then(|s| s.get(k)).and_then(Value::as_i64);
        Ok(obj! {
            "version" => version,
            "username" => v.get("user").and_then(|u| u.get("username")).and_then(Value::as_str).map(str::to_string),
            "connected" => flag("isConnected", "Connected"),
            "loggedIn" => flag("isLoggedIn", "LoggedIn"),
            "sharedFolders" => count("directories"),
            "sharedFiles" => count("files"),
        })
    }

    /// Starts a search for these words.
    pub fn search(&self, app: &App, text: &str) -> Result<Value, SlskdError> {
        let v = Self::json(&self.call(app, "POST", "searches", Some(obj! { "searchText" => text }))?)?;
        let id = v.get("id").and_then(Value::as_str).filter(|i| is_guid(i)).ok_or_else(|| SlskdError::new("slskd_failed", "slskd did not start the search."))?;
        Ok(obj! { "id" => id, "text" => text })
    }

    /// Where a search stands, with every folder of its results (best first).
    pub fn search_state(&self, app: &App, id: &str, lossless_only: bool) -> Result<SearchState, SlskdError> {
        if !is_guid(id) {
            return Err(SlskdError::new("invalid_params", "id must be a slskd search id"));
        }
        let v = Self::json(&self.call(app, "GET", &format!("searches/{id}?includeResponses=true"), None)?)?;
        let flags = v.get("state").and_then(Value::as_str).unwrap_or("");
        let has = |f: &str| flags.split(',').any(|p| p.trim() == f);
        // slskd stores a search's responses only once it has ended: until
        // `endedAt` is there it reports how many people answered (and for a
        // moment "Completed" with no responses yet).
        let ended = v.get("endedAt").is_some_and(|e| !e.is_null());
        let state = if !ended {
            "searching"
        } else if has("Errored") {
            "failed"
        } else if has("Cancelled") {
            "stopped"
        } else {
            "done"
        };
        let response_count = v.get("responseCount").and_then(Value::as_i64).unwrap_or(0);
        let mut responses = if ended { v.get("responses").and_then(Value::as_array).cloned().unwrap_or_default() } else { Vec::new() };
        if ended && responses.is_empty() && response_count > 0 {
            // Versions of slskd that send responses only on their own.
            let r = Self::json(&self.call(app, "GET", &format!("searches/{id}/responses"), None)?)?;
            responses = r.as_array().cloned().unwrap_or_default();
        }
        let folders = group(&responses, lossless_only);
        let hidden: usize = folders.iter().map(|f| f.hidden).sum::<usize>() + hidden_in_dropped(&responses, lossless_only, &folders);
        Ok(SearchState { state, text: v.get("searchText").and_then(Value::as_str).unwrap_or("").to_string(), responses: response_count, folders, hidden })
    }

    /// Where a search stands, and its results grouped into folders, as the
    /// interface shows them (the best `MAX_FOLDERS`).
    pub fn results(&self, app: &App, id: &str, lossless_only: bool) -> Result<Value, SlskdError> {
        let SearchState { state, text, responses: response_count, folders, hidden } = self.search_state(app, id, lossless_only)?;
        let total_folders = folders.len();
        let mut shown = Vec::new();
        let mut files = 0usize;
        for f in folders {
            if shown.len() >= MAX_FOLDERS || (files > 0 && files + f.files.len() > MAX_FILES) {
                break;
            }
            files += f.files.len();
            shown.push(f);
        }
        let owned = in_library(&app.db.read(), &shown)?;
        let list: Vec<Value> = shown
            .iter()
            .map(|f| {
                obj! {
                    "username" => f.username.as_str(),
                    "directory" => f.directory.as_str(),
                    "name" => f.name.as_str(),
                    "freeSlot" => f.free_slot,
                    "queue" => f.queue,
                    "speed" => f.speed,
                    "size" => f.files.iter().map(|x| x.size).sum::<i64>(),
                    "hidden" => f.hidden,
                    "files" => Value::Array(f.files.iter().map(|x| obj! {
                        "filename" => x.filename.as_str(),
                        "name" => x.name.as_str(),
                        "ext" => x.ext.as_str(),
                        "size" => x.size,
                        "bitDepth" => x.bit_depth,
                        "sampleRate" => x.sample_rate,
                        "bitRate" => x.bit_rate,
                        "length" => x.length,
                        "lossless" => x.lossless,
                        "inLibrary" => owned.contains(&(plain(&x.name), x.size)),
                    }).collect()),
                }
            })
            .collect();
        Ok(obj! {
            "id" => id,
            "text" => text,
            "state" => state,
            "responses" => response_count,
            "folders" => Value::Array(list),
            "moreFolders" => total_folders.saturating_sub(shown.len()),
            "hiddenFiles" => hidden,
        })
    }

    /// Stops a search early.
    pub fn stop(&self, app: &App, id: &str) -> Result<(), SlskdError> {
        if !is_guid(id) {
            return Err(SlskdError::new("invalid_params", "id must be a slskd search id"));
        }
        self.call(app, "PUT", &format!("searches/{id}"), None).map(|_| ())
    }

    /// Asks slskd to download these files (remote path and size) from one user.
    pub fn download(&self, app: &App, username: &str, files: &[(String, i64)]) -> Result<Value, SlskdError> {
        let list = Value::Array(files.iter().map(|(f, s)| obj! { "filename" => f.as_str(), "size" => *s }).collect());
        let r = self.send_patiently(app, "POST", &format!("transfers/downloads/{}", net::path_segment(username)), Some(list))?;
        if r.status == 404 || r.status == 405 {
            // Versions of slskd that take downloads only in batches.
            let body = obj! {
                "username" => username,
                "files" => Value::Array(files.iter().map(|(f, s)| obj! { "filename" => f.as_str(), "size" => *s }).collect()),
            };
            let r = self.call(app, "POST", "transfers/downloads/batches", Some(body))?;
            let failed = failed_files(sonora_json::parse(&r.body).ok().as_ref().and_then(|v| v.get("failures")));
            return Ok(obj! { "queued" => files.len().saturating_sub(failed.len()), "failed" => failed.len(), "failedFiles" => Value::Array(failed.into_iter().map(Value::String).collect()) });
        }
        if !(200..300).contains(&r.status) {
            let t = error_text(&r.body);
            return Err(match r.status {
                500 if !t.is_empty() => SlskdError::new("slskd_failed", format!("slskd could not queue the download: {t}")),
                _ => status_error(&r),
            });
        }
        let v = sonora_json::parse(&r.body).unwrap_or(Value::Null);
        let failed = failed_files(v.get("failed"));
        Ok(obj! { "queued" => files.len().saturating_sub(failed.len()), "failed" => failed.len(), "failedFiles" => Value::Array(failed.into_iter().map(Value::String).collect()) })
    }

    /// slskd's downloads, newest activity first, each with what became of
    /// the finished file in the library.
    pub fn downloads(&self, app: &App) -> Result<Value, SlskdError> {
        let v = Self::json(&self.call(app, "GET", "transfers/downloads", None)?)?;
        let mut items: Vec<Transfer> = Vec::new();
        for user in v.as_array().map(Vec::as_slice).unwrap_or(&[]) {
            let username = user.get("username").and_then(Value::as_str).unwrap_or("");
            for dir in user.get("directories").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]) {
                for t in dir.get("files").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]) {
                    // Previews are not downloads.
                    if let Some(t) = Transfer::from_json(username, t).filter(|t| !app.previews.is_hidden(&t.id)) {
                        items.push(t);
                    }
                }
            }
        }
        items.sort_by(|a, b| {
            let rank = |t: &Transfer| if t.active() { 0 } else { 1 };
            rank(a).cmp(&rank(b)).then_with(|| if a.active() { a.requested_at.cmp(&b.requested_at) } else { b.ended_at.cmp(&a.ended_at) })
        });
        let active = items.iter().filter(|t| t.active()).count();
        let more = items.len().saturating_sub(MAX_TRANSFERS);
        items.truncate(MAX_TRANSFERS);

        let c = app.db.read();
        let local = local_files(&c, &items)?;
        let track_ids: Vec<i64> = local.values().filter_map(|l| l.track_id).collect();
        let tracks: HashMap<i64, Value> = library::get_tracks(&c, &track_ids)?.into_iter().map(|t| (t.id, t.to_json())).collect();
        let list: Vec<Value> = items
            .iter()
            .map(|t| {
                let l = if t.state == "done" { local.get(&t.id) } else { None };
                obj! {
                    "id" => t.id.as_str(),
                    "username" => t.username.as_str(),
                    "filename" => t.filename.as_str(),
                    "name" => split_remote(&t.filename).1,
                    "folder" => last_segment(split_remote(&t.filename).0),
                    "size" => t.size,
                    "state" => t.state,
                    "detail" => t.detail.clone(),
                    "bytes" => t.bytes,
                    "speed" => t.speed,
                    "position" => t.position,
                    "requestedAt" => t.requested_at,
                    "endedAt" => t.ended_at,
                    "local" => l.map(|l| obj! {
                        "state" => l.state.as_str(),
                        "path" => l.path.as_str(),
                        "error" => l.error.clone(),
                        "track" => l.track_id.and_then(|id| tracks.get(&id).cloned()).unwrap_or(Value::Null),
                    }).unwrap_or(Value::Null),
                }
            })
            .collect();
        Ok(obj! { "downloads" => Value::Array(list), "active" => active, "more" => more })
    }

    /// Cancels a download (finished ones stay listed).
    pub fn cancel(&self, app: &App, username: &str, id: &str) -> Result<(), SlskdError> {
        if !is_guid(id) {
            return Err(SlskdError::new("invalid_params", "id must be a slskd transfer id"));
        }
        self.call(app, "DELETE", &format!("transfers/downloads/{}/{id}", net::path_segment(username)), None).map(|_| ())
    }

    /// Asks slskd to connect to Soulseek now (it tries again by itself, but
    /// waits up to five minutes between tries).
    pub fn connect(&self, app: &App) -> Result<(), SlskdError> {
        self.call(app, "PUT", "server", None).map(|_| ())
    }

    /// Takes finished downloads off slskd's list. The files stay where they are.
    pub fn clear_finished(&self, app: &App) -> Result<(), SlskdError> {
        self.call(app, "DELETE", "transfers/downloads/all/completed", None).map(|_| ())
    }
}

/// Where a search stands: searching, done, stopped or failed, and its
/// results so far (slskd hands them over once the search has ended).
#[derive(Clone, Debug, PartialEq)]
pub struct SearchState {
    pub state: &'static str,
    pub text: String,
    /// People who answered.
    pub responses: i64,
    pub folders: Vec<FoundFolder>,
    /// Files the format filter left out.
    pub hidden: usize,
}

/// The files slskd did not queue: a list of names, or of objects with a
/// `filename` (slskd answers either way, by version and route).
fn failed_files(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string).or_else(|| x.get("filename").and_then(Value::as_str).map(str::to_string))).collect())
        .unwrap_or_default()
}

/// What SONORA needs from slskd's settings (`GET options`).
#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    pub downloads: PathBuf,
    pub incomplete: PathBuf,
    /// slskd resumes partial downloads (its default) rather than starting them over.
    pub resumes: bool,
}

/// One download, as `GET transfers/downloads/{user}/{id}` describes it.
#[derive(Clone, Debug, PartialEq)]
pub struct TransferInfo {
    pub id: String,
    pub filename: String,
    pub size: i64,
    /// slskd's flags ("InProgress", "Completed, Succeeded"…).
    pub flags: String,
    pub state: &'static str,
    pub bytes: i64,
    pub position: Option<i64>,
    pub batch: Option<String>,
    pub detail: Option<String>,
}

impl TransferInfo {
    fn from_json(t: &Value) -> Option<TransferInfo> {
        let flags = t.get("state").and_then(Value::as_str).unwrap_or("").to_string();
        let state = transfer_state(&flags);
        Some(TransferInfo {
            id: t.get("id").and_then(Value::as_str).filter(|i| is_guid(i))?.to_string(),
            filename: t.get("filename").and_then(Value::as_str).unwrap_or("").to_string(),
            size: t.get("size").and_then(Value::as_i64).unwrap_or(0),
            state,
            bytes: t.get("bytesTransferred").and_then(Value::as_i64).unwrap_or(0),
            position: t.get("placeInQueue").and_then(Value::as_i64),
            batch: t.get("batchId").and_then(Value::as_str).map(str::to_string),
            detail: if state == "failed" { Some(failure(&flags, t.get("exception").and_then(Value::as_str))) } else { None },
            flags,
        })
    }

    pub fn active(&self) -> bool {
        !matches!(self.state, "done" | "failed" | "cancelled")
    }

    /// The other user turned it down: asking again does not help.
    pub fn rejected(&self) -> bool {
        self.flags.split(',').any(|f| f.trim() == "Rejected")
    }
}

/// A random identifier in the form slskd takes (a version 4 GUID).
pub fn new_guid() -> String {
    let mut b = crate::secrets::random_bytes(16).unwrap_or_else(|_| {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        t.to_le_bytes().to_vec()
    });
    b.resize(16, 0);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

impl Slskd {
    /// Where slskd keeps downloads, finished and partial, and whether it resumes them.
    pub fn options(&self, app: &App) -> Result<Options, SlskdError> {
        let v = Self::json(&self.call(app, "GET", "options", None)?)?;
        let dirs = v.get("directories");
        let dir = |k: &str| dirs.and_then(|d| d.get(k)).and_then(Value::as_str).filter(|p| !p.trim().is_empty()).map(PathBuf::from);
        let partial = v
            .get("transfers")
            .and_then(|t| t.get("download"))
            .and_then(|d| d.get("retry"))
            .and_then(|r| r.get("partial"))
            .and_then(Value::as_str)
            .unwrap_or("resume");
        Ok(Options {
            downloads: dir("downloads").ok_or_else(|| SlskdError::new("slskd_failed", "slskd did not say where it keeps downloads."))?,
            incomplete: dir("incomplete").ok_or_else(|| SlskdError::new("slskd_failed", "slskd did not say where it keeps partial downloads."))?,
            resumes: partial.eq_ignore_ascii_case("resume"),
        })
    }

    /// Asks slskd for one file as a batch of its own that lands in
    /// `destination` (relative to slskd's download folder). The transfer's
    /// id, once slskd has it.
    pub fn enqueue_batch(&self, app: &App, batch: &str, username: &str, filename: &str, size: i64, destination: &str) -> Result<Option<String>, SlskdError> {
        let body = obj! {
            "id" => batch,
            "username" => username,
            "files" => Value::Array(vec![obj! { "filename" => filename, "size" => size }]),
            "options" => obj! { "destination" => destination },
        };
        let r = self.send_patiently(app, "POST", "transfers/downloads/batches", Some(body))?;
        if r.status == 404 && error_text(&r.body).is_empty() {
            return Err(SlskdError::new("slskd_too_old", "This slskd cannot keep previews apart from downloads. Update slskd to 0.24 or newer."));
        }
        if !(200..300).contains(&r.status) {
            let t = error_text(&r.body);
            return Err(match r.status {
                404 | 500 if !t.is_empty() => SlskdError::new("slskd_failed", t),
                _ => status_error(&r),
            });
        }
        let v = sonora_json::parse(&r.body).unwrap_or(Value::Null);
        if let Some(f) = v.get("failures").and_then(Value::as_array).and_then(|a| a.first()) {
            let m = f.get("message").and_then(Value::as_str).unwrap_or("slskd did not queue it");
            return Err(SlskdError::new("slskd_failed", error_text(m)));
        }
        let id = v
            .get("batch")
            .and_then(|b| b.get("transfers"))
            .and_then(Value::as_array)
            .and_then(|a| a.iter().find(|t| t.get("filename").and_then(Value::as_str) == Some(filename)))
            .and_then(|t| t.get("id"))
            .and_then(Value::as_str)
            .filter(|i| is_guid(i))
            .map(str::to_string);
        Ok(id)
    }

    /// One user's downloads (`None` when slskd has none from them).
    pub fn user_transfers(&self, app: &App, username: &str) -> Result<Vec<TransferInfo>, SlskdError> {
        let r = self.send_patiently(app, "GET", &format!("transfers/downloads/{}", net::path_segment(username)), None)?;
        if r.status == 404 {
            return Ok(Vec::new());
        }
        if !(200..300).contains(&r.status) {
            return Err(status_error(&r));
        }
        let v = Self::json(&r)?;
        let mut out = Vec::new();
        for dir in v.get("directories").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]) {
            for t in dir.get("files").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]) {
                if let Some(t) = TransferInfo::from_json(t) {
                    out.push(t);
                }
            }
        }
        Ok(out)
    }

    /// One download (`None` when slskd no longer has it).
    pub fn transfer(&self, app: &App, username: &str, id: &str) -> Result<Option<TransferInfo>, SlskdError> {
        if !is_guid(id) {
            return Err(SlskdError::new("invalid_params", "id must be a slskd transfer id"));
        }
        let r = self.send_patiently(app, "GET", &format!("transfers/downloads/{}/{id}", net::path_segment(username)), None)?;
        if r.status == 404 {
            return Ok(None);
        }
        if !(200..300).contains(&r.status) {
            return Err(status_error(&r));
        }
        Ok(TransferInfo::from_json(&Self::json(&r)?))
    }

    /// Cancels a download and, with `remove`, takes it off slskd's list.
    /// One that is not there any more is not an error.
    pub fn cancel_transfer(&self, app: &App, username: &str, id: &str, remove: bool) -> Result<(), SlskdError> {
        if !is_guid(id) {
            return Err(SlskdError::new("invalid_params", "id must be a slskd transfer id"));
        }
        let q = if remove { "?remove=true" } else { "" };
        let r = self.send_patiently(app, "DELETE", &format!("transfers/downloads/{}/{id}{q}", net::path_segment(username)), None)?;
        if (200..300).contains(&r.status) || r.status == 404 {
            Ok(())
        } else {
            Err(status_error(&r))
        }
    }
}

/// Files the format filter left out in folders that ended up with none.
fn hidden_in_dropped(responses: &[Value], lossless_only: bool, kept: &[FoundFolder]) -> usize {
    if !lossless_only {
        return 0;
    }
    let kept: HashSet<(&str, &str)> = kept.iter().map(|f| (f.username.as_str(), f.directory.as_str())).collect();
    let mut n = 0;
    for r in responses {
        let username = r.get("username").and_then(Value::as_str).unwrap_or("");
        for f in r.get("files").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]) {
            if let Some(file) = found_file(f).filter(is_audio) {
                if !file.lossless && !kept.contains(&(username, split_remote(&file.filename).0)) {
                    n += 1;
                }
            }
        }
    }
    n
}

/// Files in the results that the library already has: same name (as
/// letters and digits) and same size.
fn in_library(c: &Connection, folders: &[FoundFolder]) -> Result<HashSet<(String, i64)>, StoreError> {
    let sizes: Vec<i64> = folders.iter().flat_map(|f| f.files.iter().map(|x| x.size)).collect::<HashSet<_>>().into_iter().collect();
    let mut out = HashSet::new();
    for chunk in sizes.chunks(400) {
        let marks = vec!["?"; chunk.len()].join(",");
        let p: Vec<SqlValue> = chunk.iter().map(|&s| SqlValue::Int(s)).collect();
        for (name, size) in c.query(&format!("SELECT file_name, size FROM tracks WHERE size IN ({marks})"), &as_params(&p), |r| (r.text(0), r.i64(1)))? {
            out.insert((plain(&name), size));
        }
    }
    Ok(out)
}

/// One download as slskd lists it.
#[derive(Clone, Debug)]
struct Transfer {
    id: String,
    username: String,
    filename: String,
    size: i64,
    state: &'static str,
    detail: Option<String>,
    bytes: i64,
    speed: f64,
    position: Option<i64>,
    requested_at: Option<i64>,
    ended_at: Option<i64>,
}

impl Transfer {
    fn from_json(username: &str, t: &Value) -> Option<Transfer> {
        let id = t.get("id").and_then(Value::as_str).filter(|i| is_guid(i))?.to_string();
        let filename = t.get("filename").and_then(Value::as_str)?.to_string();
        let flags = t.get("state").and_then(Value::as_str).unwrap_or("");
        let state = transfer_state(flags);
        let time = |k: &str| t.get(k).and_then(Value::as_str).and_then(parse_time);
        Some(Transfer {
            username: t.get("username").and_then(Value::as_str).unwrap_or(username).to_string(),
            size: t.get("size").and_then(Value::as_i64).unwrap_or(0),
            detail: if state == "failed" { Some(failure(flags, t.get("exception").and_then(Value::as_str))) } else { None },
            bytes: t.get("bytesTransferred").and_then(Value::as_i64).unwrap_or(0),
            speed: t.get("averageSpeed").and_then(Value::as_f64).unwrap_or(0.0),
            position: t.get("placeInQueue").and_then(Value::as_i64),
            requested_at: time("requestedAt").or_else(|| time("enqueuedAt")),
            ended_at: time("endedAt"),
            id,
            filename,
            state,
        })
    }

    fn active(&self) -> bool {
        !matches!(self.state, "done" | "failed" | "cancelled")
    }
}

/// A finished download as SONORA found it in the download folder.
#[derive(Clone, Debug)]
pub struct LocalFile {
    pub path: String,
    /// `review`, `importing`, `imported`, `failed` or `declined`.
    pub state: String,
    pub track_id: Option<i64>,
    pub error: Option<String>,
}

/// The file a finished transfer became in the download folder, if the
/// watcher has found it: the same size, and the same name as letters and
/// digits (or that name with a suffix).
pub fn local_file(c: &Connection, filename: &str, size: i64) -> Result<Option<LocalFile>, StoreError> {
    let t = Transfer { id: String::new(), username: String::new(), filename: filename.to_string(), size, state: "done", detail: None, bytes: size, speed: 0.0, position: None, requested_at: None, ended_at: None };
    Ok(local_files(c, std::slice::from_ref(&t))?.into_values().next())
}

/// Matches finished downloads to the files the folder watcher found: the
/// same size, and the same name as letters and digits (clients replace some
/// characters when saving, and may add a suffix when a name is taken).
fn local_files(c: &Connection, items: &[Transfer]) -> Result<HashMap<String, LocalFile>, StoreError> {
    let done: Vec<&Transfer> = items.iter().filter(|t| t.state == "done").collect();
    let sizes: Vec<i64> = done.iter().map(|t| t.size).collect::<HashSet<_>>().into_iter().collect();
    let mut by_size: HashMap<i64, Vec<LocalFile>> = HashMap::new();
    for chunk in sizes.chunks(400) {
        let marks = vec!["?"; chunk.len()].join(",");
        let mut p: Vec<SqlValue> = vec![SqlValue::Text(crate::sources::SOULSEEK.to_string())];
        p.extend(chunk.iter().map(|&s| SqlValue::Int(s)));
        let rows = c.query(
            &format!("SELECT path, size, state, track_id, error FROM source_files WHERE source = ? AND size IN ({marks}) ORDER BY found_at DESC"),
            &as_params(&p),
            |r| (r.i64(1), LocalFile { path: r.text(0), state: r.text(2), track_id: r.opt_i64(3), error: r.opt_text(4) }),
        )?;
        for (size, l) in rows {
            by_size.entry(size).or_default().push(l);
        }
    }
    let mut out = HashMap::new();
    for t in done {
        let Some(candidates) = by_size.get(&t.size) else { continue };
        let remote = split_remote(&t.filename).1;
        let (want, want_stem) = (plain(remote), plain(stem(remote)));
        let name_of = |l: &LocalFile| Path::new(&l.path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let found = candidates.iter().find(|l| plain(&name_of(l)) == want).or_else(|| {
            candidates.iter().find(|l| {
                let n = name_of(l);
                !want_stem.is_empty() && plain(stem(&n)).starts_with(&want_stem) && extension(&n) == extension(remote)
            })
        });
        if let Some(l) = found {
            out.insert(t.id.clone(), l.clone());
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, size: i64) -> Value {
        obj! { "filename" => name, "size" => size, "bitDepth" => 16, "sampleRate" => 44100, "length" => 300 }
    }

    #[test]
    fn addresses_are_checked() {
        assert_eq!(normalize_url(" http://localhost:5030/ ").unwrap(), "http://localhost:5030");
        assert_eq!(normalize_url("https://nas.local/slskd").unwrap(), "https://nas.local/slskd");
        for bad in ["localhost:5030", "ftp://x", "http://", "http://:5030", "http://user:pw@host", "http://host/a b", "http://host/?q=1"] {
            assert!(normalize_url(bad).is_err(), "{bad}");
        }
        assert!(is_local("http://localhost:5030"));
        assert!(is_local("http://127.0.0.1:5030/x"));
        assert!(is_local("http://[::1]:5030"));
        assert!(!is_local("http://192.168.1.4:5030"));
        assert!(!is_local("http://localhost.example.com"));
    }

    #[test]
    fn keys_and_ids_are_checked() {
        assert!(check_key("0123456789abcdef").is_ok());
        assert!(check_key("short").is_err());
        assert!(check_key("0123456789abcdef with space").is_err());
        assert!(is_guid("3f2504e0-4f89-11d3-9a0c-0305e82c3301"));
        assert!(!is_guid("3f2504e0-4f89-11d3-9a0c-0305e82c330"));
        assert!(!is_guid("../../../etc/passwd-aaaaaaaaaaaaaaaaaa"));
    }

    #[test]
    fn times_are_read_in_utc() {
        assert_eq!(parse_time("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_time("1970-01-01T00:00:01.5"), Some(1500));
        assert_eq!(parse_time("2026-10-20T10:00:00.1234567+04:00"), parse_time("2026-10-20T06:00:00.123Z"));
        assert_eq!(parse_time("2000-03-01T00:00:00Z"), Some(951_868_800_000));
        assert_eq!(parse_time("yesterday"), None);
    }

    #[test]
    fn results_are_grouped_into_folders_best_first() {
        let responses = vec![
            obj! {
                "username" => "slow", "hasFreeUploadSlot" => false, "queueLength" => 12, "uploadSpeed" => 900_000,
                "files" => Value::Array(vec![file("@@a\\Music\\Artist\\Album\\01 One.flac", 30), file("@@a\\Music\\Artist\\Album\\02 Two.flac", 31)]),
            },
            obj! {
                "username" => "fast", "hasFreeUploadSlot" => true, "queueLength" => 0, "uploadSpeed" => 2_000_000,
                "files" => Value::Array(vec![
                    file("Shared\\Artist - Album\\02 Two.mp3", 10),
                    file("Shared\\Artist - Album\\01 One.flac", 30),
                    file("Shared\\Artist - Album\\cover.jpg", 3),
                    file("Shared\\Other\\x.mp3", 5),
                    file("Shared\\Other\\notes.nfo", 1),
                ]),
                "lockedFiles" => Value::Array(vec![file("Shared\\Locked\\y.flac", 7)]),
            },
        ];
        let folders = group(&responses, true);
        assert_eq!(folders.len(), 2, "{folders:?}");
        assert_eq!((folders[0].username.as_str(), folders[0].name.as_str()), ("fast", "Artist - Album"));
        assert_eq!(folders[0].files.len(), 1);
        assert_eq!(folders[0].hidden, 1, "the MP3; the cover is not audio and not counted");
        assert_eq!(folders[1].files.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["01 One.flac", "02 Two.flac"]);
        assert_eq!(hidden_in_dropped(&responses, true, &folders), 1, "the folder of MP3s");
        let f = &folders[0].files[0];
        assert_eq!((f.ext.as_str(), f.lossless, f.bit_depth, f.sample_rate), ("flac", true, Some(16), Some(44100)));
        let all = group(&responses, false);
        assert_eq!(all.len(), 3);
        assert_eq!(all.iter().map(|f| f.files.len()).sum::<usize>(), 5, "locked files never listed");
    }

    #[test]
    fn transfer_states_read_as_words() {
        assert_eq!(transfer_state("Completed, Succeeded"), "done");
        assert_eq!(transfer_state("Completed, Cancelled"), "cancelled");
        assert_eq!(transfer_state("Completed, Rejected"), "failed");
        assert_eq!(transfer_state("Completed, Errored"), "failed");
        assert_eq!(transfer_state("Completed, TimedOut"), "failed");
        assert_eq!(transfer_state("InProgress"), "downloading");
        assert_eq!(transfer_state("Initializing"), "starting");
        assert_eq!(transfer_state("Queued, Remotely"), "queued");
        assert_eq!(transfer_state("Queued, Locally"), "waiting");
        assert_eq!(transfer_state("Requested"), "requested");
        assert_eq!(transfer_state("None"), "requested");
        assert_eq!(failure("Completed, Rejected", Some("File not shared.")), "The other user turned the request down: File not shared.");
        assert_eq!(failure("Completed, Errored", None), "The download failed");
    }

    #[test]
    fn saved_names_are_matched_loosely() {
        assert_eq!(plain("01 Track: Name (Mix).flac"), plain("01 Track_ Name (Mix).flac"));
        assert!(plain(stem("01 Song_639012.flac")).starts_with(&plain(stem("01 Song.flac"))));
        assert_eq!(split_remote("@@x\\a\\b.flac"), ("@@x\\a", "b.flac"));
        assert_eq!(split_remote("b.flac"), ("", "b.flac"));
        assert_eq!(last_segment("@@x\\Music\\Album"), "Album");
        assert_eq!(error_text("\"User x appears to be offline\""), "User x appears to be offline");
        assert_eq!(error_text("plain words\n"), "plain words");
    }
}
