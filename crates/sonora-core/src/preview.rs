//! Listening to a Soulseek file before downloading it.
//!
//! Soulseek has no streaming: a file comes from the other person's client in
//! one transfer, from the start (or from where an earlier transfer stopped).
//! slskd writes what arrives into a partial file in its folder for partial
//! downloads, and resumes from its length when asked for the file again.
//! A preview uses exactly that:
//!
//! 1. SONORA asks slskd for the file as a batch of its own, to be saved in
//!    `.sonora-preview/<id>` inside slskd's download folder should it ever
//!    arrive whole (SONORA's folder watcher never looks there).
//! 2. It reads slskd's partial file as it grows and decodes it (FLAC, WAV
//!    and AIFF are decoded natively, as they arrive) into a small cache of
//!    its own, served to the player as a plain PCM WAV. Playback starts as
//!    soon as a couple of seconds are there.
//! 3. It keeps no more than [`Settings::preview_ahead_s`] seconds ahead of
//!    what plays: further ahead, it cancels the transfer; nearer, it asks
//!    for the file again and slskd resumes where it stopped.
//! 4. When the preview stops, SONORA cancels the transfer, takes it off
//!    slskd's list and removes the partial file it made (never one that was
//!    there before), the preview folder, and its own cache file.
//!
//! A preview is never added to the library, never analysed, never kept and
//! never shared (slskd shares only the folders in its settings). Choosing
//! Download while listening hands over: the preview's transfer stops, its
//! partial file stays, and slskd's ordinary download resumes from it.
//!
//! Seeking works within what has arrived. What the other client reports
//! about a file is not checked: the preview plays whatever comes.

use crate::app::App;
use crate::logging;
use crate::slskd::{self, SlskdError, TransferInfo};
use sonora_engine::decode::aiff::AiffSource;
use sonora_engine::decode::flac::FlacSource;
use sonora_engine::decode::wav::WavSource;
use sonora_engine::decode::PcmSource;
use sonora_json::{obj, Value};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// The folder (in slskd's download folder) where a preview lands if it
/// arrives whole before it is stopped. Hidden, and skipped by the watcher.
pub const PREVIEW_DIR: &str = ".sonora-preview";
/// Playback starts once this much is decoded.
const START_SECONDS: f64 = 1.5;
/// Attempts after a transfer fails (the other user's client went away, say).
const RETRIES: u32 = 3;
/// On Windows slskd keeps its partial file locked while it writes: the
/// transfer stops after this many bytes so SONORA can read them.
const LOCKED_CHUNK_FIRST: u64 = 768 * 1024;
const LOCKED_CHUNK: u64 = 6 * 1024 * 1024;
/// The served WAV's header.
const HEADER: u64 = 44;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Formats a preview can be made of: decoded natively from the first byte on.
pub fn previewable(ext: &str) -> bool {
    matches!(ext.to_ascii_lowercase().as_str(), "flac" | "wav" | "wave" | "aif" | "aiff" | "aifc")
}

/// Why a format cannot be previewed, in words.
pub fn not_previewable(ext: &str) -> String {
    match ext.to_ascii_lowercase().as_str() {
        "mp3" | "ogg" | "opus" | "aac" => format!("{} files cannot be previewed yet. Previews are for FLAC, WAV and AIFF.", ext.to_ascii_uppercase()),
        "m4a" | "mp4" | "alac" => "M4A files keep what a player needs at their end, so they cannot be played before they arrive whole.".into(),
        "" => "This file has no extension, so SONORA cannot tell how to play it.".into(),
        e => format!("{} files cannot be previewed. Previews are for FLAC, WAV and AIFF.", e.to_ascii_uppercase()),
    }
}

// ----------------------------------------------------- slskd's file names

/// One path segment the way slskd makes it safe for a file name.
fn safe_segment(seg: &str, windows: bool) -> String {
    let bad = |c: char| c == '\0' || c == '/' || c == '\\' || (windows && (c < ' ' || matches!(c, '"' | '<' | '>' | '|' | ':' | '*' | '?')));
    let s: String = seg.chars().map(|c| if bad(c) { '_' } else { c }).collect();
    if s == "." || s == ".." {
        "_".into()
    } else {
        s
    }
}

/// A remote path without its root (`C:\`, `\\server\`, `@@abcde\`), as slskd strips it.
fn strip_root(p: &str) -> &str {
    let b = p.as_bytes();
    let mut s = p;
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        s = &s[2..];
        if s.starts_with('/') {
            s = &s[1..];
        }
    }
    if s.starts_with("//") {
        let rest = &s[2..];
        if let Some(i) = rest.find('/') {
            if i > 0 {
                s = &rest[i + 1..];
            }
        } else if !rest.is_empty() {
            s = "";
        }
    }
    if let Some(rest) = s.strip_prefix("@@") {
        let n = rest.bytes().take_while(u8::is_ascii_alphanumeric).count();
        if n >= 5 {
            s = &rest[n..];
            if s.starts_with('/') {
                s = &s[1..];
            }
        }
    }
    s
}

/// Where slskd keeps the partial file of `filename` from `username`:
/// `<incomplete>/<user>/<remote folders>/<name>`, each part made safe.
/// `None` when the name could lead outside that folder.
pub fn incomplete_path(root: &Path, username: &str, filename: &str, windows: bool) -> Option<PathBuf> {
    let local = filename.replace('\\', "/");
    if local.ends_with('/') || strip_root(&local).is_empty() {
        return None;
    }
    let name = safe_segment(local.rsplit('/').next()?, windows);
    let unrooted = strip_root(local.trim_end_matches('/'));
    let parts: Vec<&str> = unrooted.split('/').filter(|s| !s.is_empty()).collect();
    let dirs: Vec<String> = if parts.len() > 1 { parts[..parts.len() - 1].iter().map(|s| safe_segment(s, windows)).collect() } else { Vec::new() };
    let mut p = root.join(safe_segment(username, windows));
    for d in &dirs {
        if !d.is_empty() {
            p.push(d);
        }
    }
    if name.is_empty() {
        return None;
    }
    p.push(&name);
    let inside = p.strip_prefix(root).ok()?;
    if inside.components().any(|c| !matches!(c, Component::Normal(_))) {
        return None;
    }
    Some(p)
}

/// Where a finished download of `filename` is: slskd puts it in a folder
/// named after the one it came from, in its download folder; otherwise a
/// file with that name and size a few folders down.
fn find_finished(root: &Path, filename: &str, name: &str, size: u64, windows: bool) -> Option<PathBuf> {
    let local = filename.replace('\\', "/");
    let parent = local.rsplit('/').nth(1).filter(|p| !p.is_empty() && !p.starts_with("@@"));
    let want = safe_segment(name, windows);
    let fits = |p: &Path| std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.len() == size);
    if let Some(dir) = parent {
        let p = root.join(safe_segment(dir, windows)).join(&want);
        if fits(&p) {
            return Some(p);
        }
    }
    let mut stack = vec![(root.to_path_buf(), 0)];
    let mut seen = 0;
    while let Some((dir, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            seen += 1;
            if seen > 20_000 {
                return None;
            }
            let p = e.path();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() && depth < 3 && e.file_name() != PREVIEW_DIR {
                stack.push((p, depth + 1));
            } else if ft.is_file() && e.file_name().to_string_lossy() == want && fits(&p) {
                return Some(p);
            }
        }
    }
    None
}

/// Looks for the partial file under the user's folder when it is not where
/// slskd usually puts it (another slskd version, say): a file with that name.
fn find_partial(root: &Path, username: &str, name: &str, windows: bool) -> Option<PathBuf> {
    let want = safe_segment(name, windows);
    let mut stack = vec![(root.join(safe_segment(username, windows)), 0)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() && depth < 16 {
                stack.push((p, depth + 1));
            } else if ft.is_file() && e.file_name().to_string_lossy() == want {
                return Some(p);
            }
        }
    }
    None
}

// -------------------------------------------------------------- sessions

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Phase {
    #[default]
    Connecting,
    /// In the other user's queue (or slskd's).
    Queued,
    Downloading,
    /// Enough is read ahead: the transfer is stopped until more is needed.
    Paused,
    /// All of the file is there.
    Complete,
    /// The transfer failed; asking again.
    Reconnecting,
    Failed,
    Stopped,
}

#[derive(Default)]
struct St {
    phase: Phase,
    /// This preview asked slskd for the file (rather than following a download of it).
    owned: bool,
    /// Download was chosen while listening: slskd's ordinary download has the file now.
    handed_over: bool,
    transfer: Option<String>,
    /// Every transfer this preview made (kept off the downloads list, cleaned up).
    transfers: Vec<String>,
    incomplete: Option<PathBuf>,
    incomplete_root: Option<PathBuf>,
    /// The partial file was there before the preview (an earlier download's): it stays.
    preexisting: bool,
    preview_dir: Option<PathBuf>,
    /// slskd's folder for finished downloads.
    downloads_root: Option<PathBuf>,
    final_path: Option<PathBuf>,
    resumes: bool,
    /// Bytes of the file on disk.
    received: u64,
    /// Bytes received when the transfer last started (Windows' locked file).
    received_at_start: u64,
    /// slskd's partial file could not be opened while it was being written.
    locked: bool,
    source_complete: bool,
    /// No more bytes will come.
    source_dead: bool,
    queue: Option<i64>,
    retries: u32,
    rate: u32,
    channels: u16,
    total_frames: u64,
    decoded: u64,
    decode_done: bool,
    limit_reached: bool,
    /// Frames the interface has played.
    play: u64,
    error: Option<String>,
    unavailable: bool,
    last_emit: Option<Instant>,
}

pub struct Session {
    pub id: u64,
    pub username: String,
    pub filename: String,
    pub name: String,
    pub size: u64,
    pub ext: String,
    /// The length the other user's client reports (seconds).
    reported_secs: Option<f64>,
    pcm: PathBuf,
    st: Mutex<St>,
    cv: Condvar,
    stop: AtomicBool,
}

impl Session {
    fn notify(&self) {
        self.cv.notify_all();
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    fn block(st: &St) -> u64 {
        st.channels as u64 * 2
    }

    pub fn to_json(&self) -> Value {
        let st = lock(&self.st);
        let rate = st.rate.max(1) as f64;
        let ready = st.rate > 0 && (st.decoded as f64 / rate >= START_SECONDS.min(st.total_frames as f64 / rate) || st.decode_done);
        let state = if st.unavailable {
            "unavailable"
        } else {
            match st.phase {
                Phase::Failed => "failed",
                Phase::Stopped => "stopped",
                _ if ready => "ready",
                Phase::Queued => "queued",
                Phase::Reconnecting => "reconnecting",
                Phase::Connecting => "connecting",
                _ => "buffering",
            }
        };
        let transfer = match st.phase {
            Phase::Connecting => "connecting",
            Phase::Queued => "queued",
            Phase::Downloading => "downloading",
            Phase::Paused => "paused",
            Phase::Complete => "complete",
            Phase::Reconnecting => "reconnecting",
            Phase::Failed => "failed",
            Phase::Stopped => "stopped",
        };
        obj! {
            "id" => self.id as i64,
            "username" => self.username.as_str(),
            "filename" => self.filename.as_str(),
            "name" => self.name.as_str(),
            "size" => self.size as i64,
            "state" => state,
            "transfer" => transfer,
            "ready" => ready && !st.unavailable && st.phase != Phase::Failed,
            "queue" => st.queue,
            "received" => st.received as i64,
            "decodedSeconds" => if st.rate > 0 { Value::from(st.decoded as f64 / rate) } else { Value::Null },
            "totalSeconds" => if st.rate > 0 && st.total_frames > 0 { Value::from(st.total_frames as f64 / rate) } else { self.reported_secs.map(Value::from).unwrap_or(Value::Null) },
            "rate" => st.rate as i64,
            "channels" => st.channels as i64,
            "followsDownload" => !st.owned && !st.handed_over && st.transfer.is_some(),
            "handedOver" => st.handed_over,
            "limitReached" => st.limit_reached,
            "endsEarly" => st.source_dead && !st.source_complete && st.phase != Phase::Failed,
            "error" => st.error.clone(),
        }
    }
}

/// Every preview, and the transfers they made.
#[derive(Default)]
pub struct Previews {
    sessions: Mutex<HashMap<u64, Arc<Session>>>,
    /// Stopped previews whose transfer and files are being removed.
    cleaning: Mutex<HashMap<u64, Arc<Session>>>,
    next: AtomicU64,
    /// Transfers of previews: kept off the downloads list.
    hidden: Mutex<HashSet<String>>,
}

#[derive(Debug, PartialEq)]
pub enum ReadError {
    NotFound,
    /// The range starts at or after the end (the whole length).
    Unsatisfiable(u64),
    Failed(String),
}

/// A piece of the WAV a preview is served as.
#[derive(Debug)]
pub struct Chunk {
    pub start: u64,
    pub bytes: Vec<u8>,
    /// The whole WAV's length.
    pub total: u64,
}

fn wav_header(rate: u32, channels: u16, data: u64) -> [u8; 44] {
    let mut h = [0u8; 44];
    let block = channels as u32 * 2;
    h[0..4].copy_from_slice(b"RIFF");
    h[4..8].copy_from_slice(&((36 + data) as u32).to_le_bytes());
    h[8..12].copy_from_slice(b"WAVE");
    h[12..16].copy_from_slice(b"fmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..22].copy_from_slice(&1u16.to_le_bytes());
    h[22..24].copy_from_slice(&channels.to_le_bytes());
    h[24..28].copy_from_slice(&rate.to_le_bytes());
    h[28..32].copy_from_slice(&(rate * block).to_le_bytes());
    h[32..34].copy_from_slice(&(block as u16).to_le_bytes());
    h[34..36].copy_from_slice(&16u16.to_le_bytes());
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&(data as u32).to_le_bytes());
    h
}

/// The preview cache folder (SONORA's own).
pub fn cache_dir(app: &App) -> PathBuf {
    app.config.data_dir.join("cache").join("previews")
}

/// The file listing previews that are not cleaned up yet (in case SONORA quits first).
fn ledger(app: &App) -> PathBuf {
    cache_dir(app).join("previews.json")
}

impl Previews {
    pub fn get(&self, id: u64) -> Option<Arc<Session>> {
        lock(&self.sessions).get(&id).cloned()
    }

    /// Another preview of the same file is still being cleaned up.
    fn cleaning_same(&self, s: &Session) -> bool {
        lock(&self.cleaning).values().any(|c| c.id != s.id && c.username == s.username && c.filename == s.filename)
    }

    /// Transfers of previews (kept off the downloads list).
    pub fn is_hidden(&self, transfer: &str) -> bool {
        lock(&self.hidden).contains(transfer)
    }

    pub fn list(&self) -> Vec<Value> {
        lock(&self.sessions).values().map(|s| s.to_json()).collect()
    }

    fn emit(app: &App, s: &Session, force: bool) {
        {
            let mut st = lock(&s.st);
            if !force && st.last_emit.is_some_and(|t| t.elapsed() < Duration::from_millis(400)) {
                return;
            }
            st.last_emit = Some(Instant::now());
        }
        app.events.emit("preview.changed", &s.to_json());
    }

    /// Starts listening to `filename` from `username`. Earlier previews stop.
    pub fn start(&self, app: &Arc<App>, username: &str, filename: &str, size: u64, reported_secs: Option<f64>) -> Result<Value, SlskdError> {
        let name = filename.rsplit(['\\', '/']).next().unwrap_or(filename).to_string();
        let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
        if !previewable(&ext) {
            return Err(SlskdError { code: "preview_unavailable", message: not_previewable(&ext) });
        }
        let url = app.settings().slskd_url;
        if !slskd::is_local(&url) {
            return Err(SlskdError {
                code: "preview_unavailable",
                message: "Previews need slskd on this computer: SONORA reads what arrives from slskd's folder for partial downloads.".into(),
            });
        }
        if size == 0 {
            return Err(SlskdError { code: "preview_unavailable", message: "The other user's client did not say how big the file is.".into() });
        }
        // One preview at a time.
        let others: Vec<u64> = lock(&self.sessions).keys().copied().collect();
        for id in others {
            self.stop(app, id);
        }
        // Unique across restarts (it names the preview's folder and cache file).
        let id = crate::db::now_ms() as u64 * 1000 + self.next.fetch_add(1, Ordering::Relaxed) % 1000;
        let dir = cache_dir(app);
        std::fs::create_dir_all(&dir).map_err(|e| SlskdError { code: "preview_failed", message: format!("SONORA could not make its preview folder: {e}") })?;
        let s = Arc::new(Session {
            id,
            username: username.to_string(),
            filename: filename.to_string(),
            name,
            size,
            ext,
            reported_secs: reported_secs.filter(|v| *v > 0.0),
            pcm: dir.join(format!("preview-{id}.pcm")),
            st: Mutex::new(St { owned: true, resumes: true, ..St::default() }),
            cv: Condvar::new(),
            stop: AtomicBool::new(false),
        });
        lock(&self.sessions).insert(id, Arc::clone(&s));
        logging::log_fields("info", "preview", "preview started", obj! { "id" => id as i64, "ext" => s.ext.as_str(), "size" => size as i64 });
        {
            let (app2, s2) = (Arc::clone(app), Arc::clone(&s));
            let _ = std::thread::Builder::new().name("sonora-preview".into()).spawn(move || pump(&app2, &s2));
        }
        {
            let (app2, s2) = (Arc::clone(app), Arc::clone(&s));
            let _ = std::thread::Builder::new().name("sonora-preview-decode".into()).spawn(move || decode(&app2, &s2));
        }
        Ok(s.to_json())
    }

    /// How far the interface has played (seconds).
    pub fn position(&self, id: u64, seconds: f64) -> bool {
        let Some(s) = self.get(id) else { return false };
        let mut st = lock(&s.st);
        st.play = (seconds.max(0.0) * st.rate as f64) as u64;
        drop(st);
        s.notify();
        true
    }

    /// Stops a preview and removes what it left (in the background).
    pub fn stop(&self, app: &Arc<App>, id: u64) -> bool {
        let Some(s) = lock(&self.sessions).remove(&id) else { return false };
        lock(&self.cleaning).insert(id, Arc::clone(&s));
        s.stop.store(true, Ordering::Relaxed);
        {
            let mut st = lock(&s.st);
            if !matches!(st.phase, Phase::Failed) {
                st.phase = Phase::Stopped;
            }
        }
        s.notify();
        Self::emit(app, &s, true);
        true
    }

    /// SONORA quits: every preview stops. What they leave is removed now if
    /// there is time, otherwise at the next start (see [`recover`]).
    pub fn stop_all(&self) {
        for s in lock(&self.sessions).values() {
            s.stop.store(true, Ordering::Relaxed);
            s.notify();
        }
    }

    /// Download was chosen for files being previewed: each preview's transfer
    /// stops (its partial file stays, for slskd to resume from) and the
    /// preview follows the download from then on.
    pub fn hand_over(&self, app: &App, username: &str, filenames: &[String]) {
        let sessions: Vec<Arc<Session>> = lock(&self.sessions).values().filter(|s| s.username == username && filenames.contains(&s.filename)).cloned().collect();
        for s in sessions {
            let transfer = {
                let mut st = lock(&s.st);
                if !st.owned || st.handed_over {
                    continue;
                }
                st.handed_over = true;
                st.owned = false;
                st.transfer.take()
            };
            if let Some(t) = transfer {
                stop_transfer(app, &s.username, &t);
            }
            logging::log_fields("info", "preview", "download chosen while previewing", obj! { "id" => s.id as i64 });
            s.notify();
        }
    }

    /// Reads part of the WAV a preview is served as, waiting up to `wait`
    /// for audio that has not arrived yet.
    pub fn read(&self, id: u64, start: u64, max: u64, wait: Duration) -> Result<Chunk, ReadError> {
        let s = self.get(id).ok_or(ReadError::NotFound)?;
        let deadline = Instant::now() + wait;
        let mut st = lock(&s.st);
        loop {
            if s.stopped() {
                return Err(ReadError::Failed("The preview stopped.".into()));
            }
            if st.phase == Phase::Failed || st.unavailable {
                return Err(ReadError::Failed(st.error.clone().unwrap_or_else(|| "The preview failed.".into())));
            }
            if st.rate > 0 {
                let block = Session::block(&st);
                let total = HEADER + st.total_frames * block;
                if start >= total {
                    return Err(ReadError::Unsatisfiable(total));
                }
                // Past the decoded audio there is silence once nothing more will come.
                let avail = if st.decode_done { total } else { HEADER + st.decoded * block };
                if start < avail {
                    let end = (start + max.max(1)).min(avail);
                    let header = wav_header(st.rate, st.channels, st.total_frames * block);
                    let decoded_end = HEADER + st.decoded * block;
                    drop(st);
                    let mut bytes = Vec::with_capacity((end - start) as usize);
                    let mut pos = start;
                    if pos < HEADER {
                        let upto = end.min(HEADER);
                        bytes.extend_from_slice(&header[pos as usize..upto as usize]);
                        pos = upto;
                    }
                    if pos < end {
                        let pcm_end = end.min(decoded_end);
                        if pos < pcm_end {
                            let mut f = std::fs::File::open(&s.pcm).map_err(|e| ReadError::Failed(format!("the preview cache could not be read: {e}")))?;
                            f.seek(SeekFrom::Start(pos - HEADER)).map_err(|e| ReadError::Failed(e.to_string()))?;
                            let want = (pcm_end - pos) as usize;
                            let at = bytes.len();
                            bytes.resize(at + want, 0);
                            let mut got = 0;
                            while got < want {
                                match f.read(&mut bytes[at + got..]) {
                                    Ok(0) => break,
                                    Ok(n) => got += n,
                                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                                    Err(e) => return Err(ReadError::Failed(e.to_string())),
                                }
                            }
                            bytes.truncate(at + got);
                            pos += got as u64;
                        }
                        // Silence after the end of what could be decoded.
                        if pos < end && pos >= decoded_end {
                            bytes.resize(bytes.len() + (end - pos) as usize, 0);
                        }
                    }
                    return Ok(Chunk { start, bytes, total });
                }
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(ReadError::Failed("The preview has not got that far yet.".into()));
            }
            st = s.cv.wait_timeout(st, (deadline - now).min(Duration::from_millis(500))).map(|r| r.0).unwrap_or_else(|p| p.into_inner().0);
        }
    }
}

// -------------------------------------------------------- the transfer

/// Cancels a preview's transfer, waits until slskd let go of it, and takes
/// it off slskd's list.
fn stop_transfer(app: &App, username: &str, id: &str) {
    if app.slskd.cancel_transfer(app, username, id, false).is_err() {
        return;
    }
    let deadline = Instant::now() + Duration::from_secs(6);
    while Instant::now() < deadline {
        match app.slskd.transfer(app, username, id) {
            Ok(Some(t)) if t.active() => std::thread::sleep(Duration::from_millis(150)),
            _ => break,
        }
    }
    let _ = app.slskd.cancel_transfer(app, username, id, true);
}

fn set_phase(app: &App, s: &Session, phase: Phase) {
    let changed = {
        let mut st = lock(&s.st);
        let changed = st.phase != phase;
        st.phase = phase;
        changed
    };
    if changed {
        s.notify();
        Previews::emit(app, s, true);
    }
}

fn fail(app: &App, s: &Session, message: impl Into<String>) {
    {
        let mut st = lock(&s.st);
        st.phase = Phase::Failed;
        st.error = Some(message.into());
        st.source_dead = true;
    }
    s.notify();
    Previews::emit(app, s, true);
}

/// How far decoding is ahead of playback, and the limits (in frames).
fn ahead(app: &App, st: &St) -> Option<(u64, u64, u64)> {
    if st.rate == 0 {
        return None;
    }
    let ahead_s = app.settings().preview_ahead_s.max(10) as u64;
    let high = ahead_s * st.rate as u64;
    let low = (ahead_s / 3).max(8) * st.rate as u64;
    Some((st.decoded.saturating_sub(st.play), high, low))
}

/// Keeps the transfer going, pauses it when enough is read ahead, asks
/// again when more is needed or after a failure, and cleans up at the end.
fn pump(app: &Arc<App>, s: &Arc<Session>) {
    let windows = cfg!(windows);
    // An earlier preview of the same file finishes cleaning up first (it
    // removes the partial file this one is about to write).
    let deadline = Instant::now() + Duration::from_secs(15);
    while !s.stopped() && Instant::now() < deadline && app.previews.cleaning_same(s) {
        std::thread::sleep(Duration::from_millis(100));
    }
    match app.slskd.options(app) {
        Ok(o) => {
            let Some(partial) = incomplete_path(&o.incomplete, &s.username, &s.filename, windows) else {
                fail(app, s, "This file's name cannot be used safely on this computer.");
                cleanup(app, s);
                return;
            };
            let mut st = lock(&s.st);
            st.preexisting = partial.exists();
            st.received = std::fs::metadata(&partial).map(|m| m.len()).unwrap_or(0);
            st.incomplete = Some(partial);
            st.incomplete_root = Some(o.incomplete.clone());
            st.preview_dir = Some(o.downloads.join(PREVIEW_DIR).join(s.id.to_string()));
            st.downloads_root = Some(o.downloads.clone());
            st.resumes = o.resumes;
        }
        Err(e) => {
            fail(app, s, e.message);
            cleanup(app, s);
            return;
        }
    }
    // A download of this very file is under way: listen along, without touching it.
    if let Ok(list) = app.slskd.user_transfers(app, &s.username) {
        if let Some(t) = list.iter().find(|t| t.filename == s.filename && t.active() && !app.previews.is_hidden(&t.id)) {
            let mut st = lock(&s.st);
            st.owned = false;
            st.transfer = Some(t.id.clone());
        }
    }
    let mut last_poll = Instant::now() - Duration::from_secs(1);
    let mut backoff_until: Option<Instant> = None;
    let mut unreachable_since: Option<Instant> = None;
    loop {
        if s.stopped() {
            break;
        }
        update_received(s, windows);
        let (owned, handed_over, transfer, phase) = {
            let st = lock(&s.st);
            (st.owned, st.handed_over, st.transfer.clone(), st.phase)
        };
        if matches!(phase, Phase::Failed) {
            // Wait for the interface to stop it.
            wait(s, 300);
            continue;
        }
        if backoff_until.is_some_and(|t| Instant::now() < t) {
            wait(s, 200);
            continue;
        }
        backoff_until = None;
        match transfer {
            None if owned && !matches!(phase, Phase::Paused | Phase::Complete) && !lock(&s.st).source_dead => {
                let batch = slskd::new_guid();
                let dest = format!("{PREVIEW_DIR}/{}", s.id);
                match app.slskd.enqueue_batch(app, &batch, &s.username, &s.filename, s.size as i64, &dest) {
                    Ok(id) => {
                        let id = id.or_else(|| {
                            app.slskd.user_transfers(app, &s.username).ok()?.into_iter().find(|t| t.batch.as_deref() == Some(batch.as_str()) && t.filename == s.filename).map(|t| t.id)
                        });
                        let Some(id) = id else {
                            fail(app, s, "slskd did not start the transfer.");
                            continue;
                        };
                        lock(&app.previews.hidden).insert(id.clone());
                        {
                            let mut st = lock(&s.st);
                            st.transfer = Some(id.clone());
                            st.transfers.push(id);
                            st.received_at_start = st.received;
                        }
                        write_ledger(app);
                        if phase != Phase::Reconnecting {
                            set_phase(app, s, Phase::Connecting);
                        }
                    }
                    Err(e) if e.code == "slskd_unreachable" || e.code == "slskd_busy" => {
                        set_phase(app, s, Phase::Reconnecting);
                        backoff_until = Some(Instant::now() + Duration::from_secs(3));
                    }
                    Err(e) => fail(app, s, e.message),
                }
            }
            None => {
                // Paused (enough ahead), complete, or handed over and not followed yet.
                if handed_over && !lock(&s.st).source_complete {
                    if let Ok(list) = app.slskd.user_transfers(app, &s.username) {
                        if let Some(t) = list.iter().find(|t| t.filename == s.filename && t.active() && !app.previews.is_hidden(&t.id)) {
                            lock(&s.st).transfer = Some(t.id.clone());
                        }
                    }
                    wait(s, 400);
                    continue;
                }
                if phase == Phase::Paused {
                    let resume = {
                        let st = lock(&s.st);
                        let need_more = ahead(app, &st).is_some_and(|(a, _, low)| a < low);
                        need_more && st.resumes && !st.source_dead && !st.source_complete && !st.limit_reached
                    };
                    if resume {
                        set_phase(app, s, Phase::Connecting);
                        continue;
                    }
                }
                wait(s, 250);
            }
            Some(id) => {
                if last_poll.elapsed() < Duration::from_millis(400) {
                    wait(s, 100);
                } else {
                    last_poll = Instant::now();
                    match app.slskd.transfer(app, &s.username, &id) {
                        Ok(Some(t)) => {
                            unreachable_since = None;
                            on_transfer(app, s, &t, &mut backoff_until);
                        }
                        Ok(None) => {
                            unreachable_since = None;
                            // Taken off slskd's list (in slskd's own page, say).
                            lock(&s.st).transfer = None;
                            if !owned {
                                if update_received(s, windows) >= s.size {
                                    lock(&s.st).source_complete = true;
                                    set_phase(app, s, Phase::Complete);
                                } else {
                                    lock(&s.st).source_dead = true;
                                }
                            }
                        }
                        Err(e) if e.code == "slskd_unreachable" => {
                            let since = *unreachable_since.get_or_insert_with(Instant::now);
                            set_phase(app, s, Phase::Reconnecting);
                            if since.elapsed() > Duration::from_secs(60) {
                                fail(app, s, e.message);
                            }
                        }
                        Err(_) => {}
                    }
                }
                control_read_ahead(app, s);
            }
        }
        Previews::emit(app, s, false);
    }
    cleanup(app, s);
}

fn wait(s: &Session, ms: u64) {
    let st = lock(&s.st);
    let _ = s.cv.wait_timeout(st, Duration::from_millis(ms));
}

/// What slskd says about the transfer, as the preview's phase.
fn on_transfer(app: &App, s: &Arc<Session>, t: &TransferInfo, backoff: &mut Option<Instant>) {
    {
        let mut st = lock(&s.st);
        st.queue = t.position;
    }
    match t.state {
        "requested" | "starting" => set_phase(app, s, Phase::Connecting),
        "queued" | "waiting" => set_phase(app, s, Phase::Queued),
        "downloading" => set_phase(app, s, Phase::Downloading),
        "done" => {
            {
                let mut st = lock(&s.st);
                st.transfer = None;
                st.source_complete = true;
                // A preview's file moved into its own folder; a followed
                // download's into the download folder.
                if st.final_path.is_none() {
                    if let Some(dir) = st.preview_dir.clone() {
                        st.final_path = std::fs::read_dir(&dir).ok().and_then(|rd| rd.flatten().map(|e| e.path()).find(|p| p.is_file()));
                    }
                }
                if st.final_path.is_none() && !st.incomplete.as_ref().is_some_and(|p| p.exists()) {
                    if let Some(root) = st.downloads_root.clone() {
                        st.final_path = find_finished(&root, &s.filename, &s.name, s.size, cfg!(windows));
                    }
                }
            }
            set_phase(app, s, Phase::Complete);
        }
        "cancelled" => {
            // Cancelled outside SONORA: nothing more will come.
            let mut st = lock(&s.st);
            st.transfer = None;
            if st.owned {
                st.source_dead = true;
                st.error = Some("The transfer was stopped in slskd.".into());
            }
        }
        _ => {
            // Failed: try again from where it stopped, unless the other user said no.
            let retry = {
                let mut st = lock(&s.st);
                st.transfer = None;
                st.retries += 1;
                st.owned && !t.rejected() && st.retries <= RETRIES && st.resumes
            };
            if retry {
                set_phase(app, s, Phase::Reconnecting);
                let n = lock(&s.st).retries as u64;
                *backoff = Some(Instant::now() + Duration::from_secs(2 * n * n));
            } else {
                fail(app, s, t.detail.clone().unwrap_or_else(|| "The transfer failed.".into()));
            }
        }
    }
}

/// Stops the transfer when enough is read ahead (or the cache is full, or
/// on Windows when a piece of the locked file is ready to read).
fn control_read_ahead(app: &App, s: &Arc<Session>) {
    let pause = {
        let st = lock(&s.st);
        if !st.owned || st.handed_over || st.phase != Phase::Downloading {
            return;
        }
        let full = ahead(app, &st).is_some_and(|(a, high, _)| a > high);
        let chunk = if st.decoded == 0 { LOCKED_CHUNK_FIRST } else { LOCKED_CHUNK };
        let locked_piece = st.locked && st.received.saturating_sub(st.received_at_start) >= chunk;
        full || st.limit_reached || locked_piece
    };
    if !pause {
        return;
    }
    let id = lock(&s.st).transfer.take();
    if let Some(id) = id {
        stop_transfer(app, &s.username, &id);
    }
    {
        let mut st = lock(&s.st);
        // slskd that starts partial files over cannot be paused and resumed:
        // the preview ends where it stopped.
        if !st.resumes || st.limit_reached {
            st.source_dead = true;
        }
    }
    set_phase(app, s, Phase::Paused);
}

/// How many bytes of the file are on disk (and where).
fn update_received(s: &Session, windows: bool) -> u64 {
    let (path, root) = {
        let st = lock(&s.st);
        (st.final_path.clone().or_else(|| st.incomplete.clone()), st.incomplete_root.clone())
    };
    let Some(mut path) = path else { return 0 };
    if !path.exists() {
        let (complete, downloads) = {
            let st = lock(&s.st);
            (st.source_complete, st.downloads_root.clone())
        };
        if complete {
            // Finished and moved: into the download folder (a followed download).
            match downloads.and_then(|d| find_finished(&d, &s.filename, &s.name, s.size, windows)) {
                Some(found) => {
                    lock(&s.st).final_path = Some(found.clone());
                    path = found;
                }
                None => {
                    // Gone: the preview ends where its audio does.
                    let mut st = lock(&s.st);
                    if !st.source_dead {
                        st.source_dead = true;
                        drop(st);
                        s.notify();
                    }
                    return lock(&s.st).received;
                }
            }
        } else if let Some(root) = root {
            // Not where slskd usually puts it: look for it under the user's folder.
            if let Some(found) = find_partial(&root, &s.username, &s.name, windows) {
                lock(&s.st).incomplete = Some(found.clone());
                path = found;
            }
        }
    }
    let n = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let mut st = lock(&s.st);
    if n > st.received {
        st.received = n;
        drop(st);
        s.notify();
        return n;
    }
    st.received
}

// ------------------------------------------------------------ decoding

/// slskd's partial file, read as it grows: a read waits until the bytes are
/// there, and the stream ends where the file will end.
struct Growing {
    s: Arc<Session>,
    pos: u64,
    file: Option<std::fs::File>,
}

impl Growing {
    fn path(&self) -> Option<PathBuf> {
        let st = lock(&self.s.st);
        st.final_path.clone().or_else(|| st.incomplete.clone())
    }

    /// Bytes readable now, opening the file when needed. On Windows the
    /// file is opened for each read, as slskd locks it while writing.
    fn available(&mut self) -> u64 {
        if self.file.is_none() {
            if let Some(p) = self.path() {
                match std::fs::File::open(&p) {
                    Ok(f) => self.file = Some(f),
                    Err(e) => {
                        if p.exists() && e.kind() == std::io::ErrorKind::PermissionDenied || cfg!(windows) && p.exists() {
                            lock(&self.s.st).locked = true;
                        }
                        return 0;
                    }
                }
            }
        }
        self.file.as_ref().and_then(|f| f.metadata().ok()).map(|m| m.len()).unwrap_or(0)
    }
}

impl Read for Growing {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() || self.pos >= self.s.size {
            return Ok(0);
        }
        loop {
            if self.s.stopped() {
                return Err(std::io::Error::other("the preview stopped"));
            }
            let avail = self.available();
            if self.pos < avail {
                let f = self.file.as_mut().ok_or_else(|| std::io::Error::other("no file"))?;
                f.seek(SeekFrom::Start(self.pos))?;
                let want = buf.len().min((avail - self.pos) as usize);
                let n = f.read(&mut buf[..want])?;
                self.pos += n as u64;
                if cfg!(windows) {
                    self.file = None;
                }
                if n > 0 {
                    return Ok(n);
                }
            }
            if cfg!(windows) {
                self.file = None;
            }
            let dead = {
                let st = lock(&self.s.st);
                st.source_dead || st.phase == Phase::Failed || (st.source_complete && avail >= st.received)
            };
            if dead {
                return Ok(0);
            }
            wait(&self.s, 250);
        }
    }
}

impl Seek for Growing {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        let size = self.s.size as i64;
        let next = match to {
            SeekFrom::Start(n) => n as i64,
            SeekFrom::Current(d) => self.pos as i64 + d,
            SeekFrom::End(d) => size + d,
        };
        if next < 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "seek before the start"));
        }
        self.pos = next as u64;
        Ok(self.pos)
    }
}

fn unavailable(app: &App, s: &Session, message: impl Into<String>) {
    {
        let mut st = lock(&s.st);
        st.unavailable = true;
        st.error = Some(message.into());
        st.source_dead = true;
    }
    s.notify();
    Previews::emit(app, s, true);
}

/// Decodes what arrives into the preview cache (16-bit PCM, at most two channels).
fn decode(app: &Arc<App>, s: &Arc<Session>) {
    let reader = Box::new(Growing { s: Arc::clone(s), pos: 0, file: None });
    let opened: Result<Box<dyn PcmSource>, String> = match s.ext.as_str() {
        "flac" => FlacSource::open_reader(reader, s.size).map(|x| Box::new(x) as Box<dyn PcmSource>).map_err(|e| describe(e, "FLAC")),
        "wav" | "wave" => WavSource::open_reader(reader, s.size, true).map(|x| Box::new(x) as Box<dyn PcmSource>).map_err(describe_wav),
        _ => AiffSource::open_reader(reader, s.size, true).map(|x| Box::new(x) as Box<dyn PcmSource>).map_err(describe_aiff),
    };
    let mut src = match opened {
        Ok(src) => src,
        Err(e) => {
            // No data came (the transfer failed or stopped): that is the
            // reason to show, not that nothing could be read.
            let no_data = {
                let st = lock(&s.st);
                st.phase == Phase::Failed || (st.source_dead && !st.source_complete && st.received == 0)
            };
            if s.stopped() || no_data {
                lock(&s.st).decode_done = true;
                s.notify();
            } else {
                unavailable(app, s, e);
            }
            return;
        }
    };
    let info = src.info().clone();
    let total = match info.frames_hint.filter(|f| *f > 0).or_else(|| s.reported_secs.map(|secs| (secs * info.sample_rate as f64).round() as u64)) {
        Some(t) => t,
        None => {
            unavailable(app, s, "This file does not say how long it is, so it cannot be previewed. Download it to listen.");
            return;
        }
    };
    let channels = info.channels.clamp(1, 2);
    let block = channels as u64 * 2;
    // A WAV the player reads stays under 4 GB.
    let total = total.min((u32::MAX as u64 - HEADER) / block);
    let cap_bytes = app.settings().preview_cache_mb.max(64) as u64 * 1024 * 1024;
    let mut out = match std::fs::File::create(&s.pcm) {
        Ok(f) => f,
        Err(e) => {
            unavailable(app, s, format!("SONORA could not write its preview cache: {e}"));
            return;
        }
    };
    {
        let mut st = lock(&s.st);
        st.rate = info.sample_rate;
        st.channels = channels;
        st.total_frames = total;
    }
    s.notify();
    Previews::emit(app, s, true);
    let mut frames = Vec::new();
    let mut bytes: Vec<u8> = Vec::with_capacity(8192 * block as usize);
    let in_ch = info.channels as usize;
    let mut written = 0u64;
    loop {
        if s.stopped() {
            break;
        }
        let n = src.read(&mut frames, 8192).unwrap_or_default();
        if n == 0 {
            break;
        }
        let n = n.min((total - written) as usize);
        bytes.clear();
        for f in 0..n {
            for c in 0..channels as usize {
                let x = frames[f * in_ch + c.min(in_ch - 1)];
                let v = (x * 32767.0).round().clamp(-32768.0, 32767.0) as i16;
                bytes.extend_from_slice(&v.to_le_bytes());
            }
        }
        if out.write_all(&bytes).and_then(|_| out.flush()).is_err() {
            unavailable(app, s, "SONORA could not write its preview cache (is the disk full?).");
            return;
        }
        written += n as u64;
        {
            let mut st = lock(&s.st);
            st.decoded = written;
            if written * block >= cap_bytes {
                st.limit_reached = true;
            }
        }
        s.notify();
        Previews::emit(app, s, false);
        if written >= total || lock(&s.st).limit_reached {
            break;
        }
    }
    let _ = src.finish();
    {
        let mut st = lock(&s.st);
        st.decode_done = true;
        // Shorter than it said: the rest is silence (and the length stays as told).
        if written < total && !st.limit_reached && !st.source_complete && !st.source_dead && !s.stopped() {
            st.error = Some("The file ended early or could not be read further.".into());
        }
    }
    s.notify();
    Previews::emit(app, s, true);
}

fn describe(e: sonora_engine::decode::flac::OpenError, what: &str) -> String {
    match e {
        sonora_engine::decode::flac::OpenError::NotNative(m) => format!("This {what} cannot be previewed: {m}."),
        sonora_engine::decode::flac::OpenError::Failed(e) => format!("This file could not be read as {what} ({e})."),
    }
}

fn describe_wav(e: sonora_engine::decode::wav::OpenError) -> String {
    match e {
        sonora_engine::decode::wav::OpenError::NotNative(h) => format!("This WAV holds {} audio, which previews cannot play. Download it to listen.", h.codec_name()),
        sonora_engine::decode::wav::OpenError::Failed(e) => format!("This file could not be read as WAV ({e})."),
    }
}

fn describe_aiff(e: sonora_engine::decode::aiff::OpenError) -> String {
    match e {
        sonora_engine::decode::aiff::OpenError::NotNative(_) => "This AIFF is compressed in a way previews cannot play. Download it to listen.".into(),
        sonora_engine::decode::aiff::OpenError::Failed(e) => format!("This file could not be read as AIFF ({e})."),
    }
}

// ------------------------------------------------------------- cleaning up

/// Removes what a preview left: its transfer, the partial file it made, its
/// folder in slskd's download folder, and its cache file.
fn cleanup(app: &Arc<App>, s: &Arc<Session>) {
    let (owned, handed_over, transfer, partial, preexisting, root, dir, transfers) = {
        let st = lock(&s.st);
        (st.owned, st.handed_over, st.transfer.clone(), st.incomplete.clone(), st.preexisting, st.incomplete_root.clone(), st.preview_dir.clone(), st.transfers.clone())
    };
    if owned && !handed_over {
        if let Some(t) = &transfer {
            stop_transfer(app, &s.username, t);
        }
        // Earlier ones (finished, failed or paused) come off slskd's list too.
        for t in transfers.iter().filter(|t| Some(*t) != transfer.as_ref()) {
            let _ = app.slskd.cancel_transfer(app, &s.username, t, true);
        }
        // Other transfers of this file (a download chosen since) keep the partial file.
        let in_use = app
            .slskd
            .user_transfers(app, &s.username)
            .map(|list| list.iter().any(|t| t.filename == s.filename && t.active() && !transfers.contains(&t.id)))
            .unwrap_or(true);
        if let (Some(partial), Some(root)) = (partial, root) {
            if !preexisting && !in_use && partial.starts_with(&root) {
                remove_file_and_empty_parents(&partial, &root);
            }
        }
        if let Some(dir) = dir {
            if dir.is_dir() && dir.parent().is_some_and(|p| p.file_name().is_some_and(|n| n == PREVIEW_DIR)) {
                let _ = std::fs::remove_dir_all(&dir);
                if let Some(p) = dir.parent() {
                    let _ = std::fs::remove_dir(p);
                }
            }
        }
    }
    // The decoder lets go of the cache file once it sees the preview stopped.
    for _ in 0..30 {
        let done = {
            let st = lock(&s.st);
            st.decode_done || st.unavailable
        };
        if done {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = std::fs::remove_file(&s.pcm);
    lock(&app.previews.sessions).remove(&s.id);
    lock(&app.previews.cleaning).remove(&s.id);
    write_ledger(app);
    logging::log_fields("info", "preview", "preview cleaned up", obj! { "id" => s.id as i64, "handedOver" => handed_over });
}

/// Removes a file, then its folders up to `root` while they are empty.
fn remove_file_and_empty_parents(file: &Path, root: &Path) {
    // slskd may still be closing it.
    for _ in 0..10 {
        match std::fs::remove_file(file) {
            Ok(()) => break,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(_) => std::thread::sleep(Duration::from_millis(200)),
        }
    }
    let mut dir = file.parent();
    while let Some(d) = dir {
        if d == root || !d.starts_with(root) {
            break;
        }
        if std::fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }
}

/// Notes the previews under way, so what they leave can be removed even if
/// SONORA quits in the middle of one.
fn write_ledger(app: &App) {
    let all: Vec<Arc<Session>> = lock(&app.previews.sessions).values().chain(lock(&app.previews.cleaning).values()).cloned().collect();
    let entries: Vec<Value> = all
        .iter()
        .map(|s| {
            let st = lock(&s.st);
            obj! {
                "id" => s.id as i64,
                "username" => s.username.as_str(),
                "filename" => s.filename.as_str(),
                "transfers" => Value::Array(st.transfers.iter().map(|t| Value::String(t.clone())).collect()),
                "partial" => st.incomplete.as_ref().map(|p| p.to_string_lossy().into_owned()),
                "root" => st.incomplete_root.as_ref().map(|p| p.to_string_lossy().into_owned()),
                "preexisting" => st.preexisting,
                "dir" => st.preview_dir.as_ref().map(|p| p.to_string_lossy().into_owned()),
                "handedOver" => st.handed_over || !st.owned,
            }
        })
        .collect();
    let path = ledger(app);
    if entries.is_empty() {
        let _ = std::fs::remove_file(path);
    } else if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(path, Value::Array(entries).to_string_compact());
    }
}

/// After a restart: removes what previews left when SONORA quit during them.
pub fn recover(app: &Arc<App>) {
    let dir = cache_dir(app);
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if n.starts_with("preview-") && n.ends_with(".pcm") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let Ok(text) = std::fs::read_to_string(ledger(app)) else { return };
    let Ok(Value::Array(entries)) = sonora_json::parse(&text) else {
        let _ = std::fs::remove_file(ledger(app));
        return;
    };
    let mut left = Vec::new();
    for e in entries {
        let s = |k: &str| e.get(k).and_then(Value::as_str).map(str::to_string);
        let (Some(username), Some(filename)) = (s("username"), s("filename")) else { continue };
        let transfers: Vec<String> = e.get("transfers").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
        let handed_over = e.get("handedOver").and_then(Value::as_bool).unwrap_or(false);
        for t in &transfers {
            lock(&app.previews.hidden).insert(t.clone());
        }
        if handed_over {
            continue;
        }
        let list = match app.slskd.user_transfers(app, &username) {
            Ok(l) => l,
            Err(_) => {
                // slskd is not there: try again next time.
                left.push(e.clone());
                continue;
            }
        };
        for t in &transfers {
            if list.iter().any(|x| &x.id == t) {
                stop_transfer(app, &username, t);
            }
        }
        let in_use = list.iter().any(|t| t.filename == filename && t.active() && !transfers.contains(&t.id));
        if let (Some(partial), Some(root)) = (s("partial"), s("root")) {
            let (partial, root) = (PathBuf::from(partial), PathBuf::from(root));
            if !e.get("preexisting").and_then(Value::as_bool).unwrap_or(true) && !in_use && partial.starts_with(&root) {
                remove_file_and_empty_parents(&partial, &root);
            }
        }
        if let Some(d) = s("dir").map(PathBuf::from) {
            if d.is_dir() && d.parent().is_some_and(|p| p.file_name().is_some_and(|n| n == PREVIEW_DIR)) {
                let _ = std::fs::remove_dir_all(&d);
                if let Some(p) = d.parent() {
                    let _ = std::fs::remove_dir(p);
                }
            }
        }
    }
    if left.is_empty() {
        let _ = std::fs::remove_file(ledger(app));
    } else {
        let _ = std::fs::write(ledger(app), Value::Array(left).to_string_compact());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_files_are_where_slskd_puts_them() {
        let root = Path::new("/slskd/incomplete");
        assert_eq!(incomplete_path(root, "crate-digger", "@@abcde\\Music\\Night Drive\\01 Lights.flac", false).unwrap(), root.join("crate-digger/Music/Night Drive/01 Lights.flac"));
        assert_eq!(incomplete_path(root, "u", "C:\\Users\\x\\a.flac", false).unwrap(), root.join("u/Users/x/a.flac"));
        assert_eq!(incomplete_path(root, "u", "\\\\nas\\share\\a.flac", false).unwrap(), root.join("u/share/a.flac"));
        assert_eq!(incomplete_path(root, "u", "a.flac", false).unwrap(), root.join("u/a.flac"));
        assert_eq!(incomplete_path(root, "u", "music/sub/b.wav", false).unwrap(), root.join("u/music/sub/b.wav"));
        // Names that could lead elsewhere stay inside.
        assert_eq!(incomplete_path(root, "..", "..\\..\\x.flac", false).unwrap(), root.join("_/_/_/x.flac"));
        assert_eq!(incomplete_path(root, "a/b", "x\\y.flac", false).unwrap(), root.join("a_b/x/y.flac"));
        assert!(incomplete_path(root, "u", "folder\\", false).is_none());
        // Windows replaces more characters.
        assert_eq!(incomplete_path(root, "u", "x\\a:b?.flac", true).unwrap(), root.join("u").join("x").join("a_b_.flac"));
        // (Only where such a name is a file name: on Windows "a:" starts a drive.)
        if !cfg!(windows) {
            assert_eq!(incomplete_path(root, "u", "x\\a:b?.flac", false).unwrap(), root.join("u/x/a:b?.flac"));
        }
    }

    #[test]
    fn a_finished_download_is_found_where_slskd_puts_it() {
        let root = std::env::temp_dir().join(format!("sonora-finished-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Night Drive")).unwrap();
        std::fs::write(root.join("Night Drive").join("01 Lights.flac"), [0u8; 10]).unwrap();
        let name = "@@abcde\\Music\\Night Drive\\01 Lights.flac";
        assert_eq!(find_finished(&root, name, "01 Lights.flac", 10, false), Some(root.join("Night Drive").join("01 Lights.flac")));
        assert_eq!(find_finished(&root, name, "01 Lights.flac", 11, false), None, "another size is another file");
        // Elsewhere in the folder (another slskd, or a destination of its own).
        std::fs::create_dir_all(root.join("elsewhere").join("deeper")).unwrap();
        std::fs::write(root.join("elsewhere").join("deeper").join("02 Glow.flac"), [0u8; 7]).unwrap();
        assert_eq!(find_finished(&root, "x\\y\\02 Glow.flac", "02 Glow.flac", 7, false), Some(root.join("elsewhere").join("deeper").join("02 Glow.flac")));
        // Never a preview's own folder.
        std::fs::create_dir_all(root.join(PREVIEW_DIR).join("1")).unwrap();
        std::fs::write(root.join(PREVIEW_DIR).join("1").join("03 Hum.flac"), [0u8; 5]).unwrap();
        assert_eq!(find_finished(&root, "x\\03 Hum.flac", "03 Hum.flac", 5, false), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn formats() {
        assert!(previewable("FLAC") && previewable("wav") && previewable("aiff"));
        assert!(!previewable("mp3") && !previewable("m4a"));
        assert!(not_previewable("m4a").contains("end"));
    }

    #[test]
    fn served_header() {
        let h = wav_header(44_100, 2, 1_000);
        assert_eq!(&h[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes([h[4], h[5], h[6], h[7]]), 1_036);
        assert_eq!(u32::from_le_bytes([h[24], h[25], h[26], h[27]]), 44_100);
        assert_eq!(u16::from_le_bytes([h[32], h[33]]), 4);
        assert_eq!(u32::from_le_bytes([h[40], h[41], h[42], h[43]]), 1_000);
    }
}
