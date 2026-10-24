//! DJ-ready export of selected tracks: new AIFF, WAV or MP3 files in a folder
//! the user picks, tagged with the tempo, key and genre in use, plus an
//! optional `rekordbox.xml` that Rekordbox can import.
//!
//! The library's files are only read. Every export writes new files under
//! new names (existing files are never overwritten), through a hidden
//! temporary file that becomes the real file only once it has been
//! written completely and read back successfully.

use crate::db::sqlite::Connection;
use crate::db::StoreError;
use crate::events::EventBus;
use crate::library::get_tracks;
use crate::model::TrackRow;
use crate::params;
use sonora_engine::analysis::key::{format_key, Notation};
use sonora_engine::decode::ToolPaths;
use sonora_engine::export::convert::{convert, has_mp3_encoder, ConvertRequest, Method, Quality, Target};
use sonora_engine::export::id3::TagSet;
use sonora_engine::export::picture;
use sonora_engine::meta::probe_file;
use sonora_json::{num, obj, ToJson, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq)]
pub struct ExportOptions {
    pub target: Target,
    pub quality: Quality,
    /// Key notation written into the tags and the XML.
    pub notation: Notation,
    /// Export the best copy of each recording found in the library.
    pub best_copy: bool,
    pub rekordbox_xml: bool,
}

impl ExportOptions {
    pub fn from_json(p: &Value) -> Result<ExportOptions, String> {
        let target = Target::from_id(p.get("format").and_then(Value::as_str).unwrap_or("aiff")).ok_or("format must be aiff, wav or mp3")?;
        let quality = match p.get("quality").and_then(Value::as_str).unwrap_or("universal") {
            "universal" => Quality::Universal,
            "high" => Quality::High,
            _ => return Err("quality must be universal or high".into()),
        };
        let notation = Notation::from_id(p.get("keyNotation").and_then(Value::as_str).unwrap_or("musical")).ok_or("unknown keyNotation")?;
        Ok(ExportOptions {
            target,
            quality,
            notation,
            best_copy: p.get("bestCopy").and_then(Value::as_bool).unwrap_or(true),
            rekordbox_xml: p.get("rekordboxXml").and_then(Value::as_bool).unwrap_or(true),
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Warning {
    pub code: &'static str,
    pub message: String,
}

impl ToJson for Warning {
    fn to_json(&self) -> Value {
        obj! { "code" => self.code, "message" => self.message }
    }
}

/// What will happen to one selected track.
#[derive(Clone, Debug, PartialEq)]
pub struct PlanItem {
    pub track: TrackRow,
    /// The file actually exported (a better copy of the same recording, or the track itself).
    pub source: TrackRow,
    pub file_name: String,
    pub warnings: Vec<Warning>,
    /// Why the track is left out: "missing", "duplicate".
    pub skip: Option<&'static str>,
}

impl ToJson for PlanItem {
    fn to_json(&self) -> Value {
        obj! {
            "trackId" => self.track.id,
            "title" => self.track.display_title(),
            "artist" => self.track.artist,
            "sourceId" => self.source.id,
            "sourcePath" => self.source.path,
            "sourceFormat" => self.source.format,
            "betterCopy" => self.source.id != self.track.id,
            "fileName" => self.file_name,
            "warnings" => Value::Array(self.warnings.iter().map(ToJson::to_json).collect()),
            "skip" => self.skip,
        }
    }
}

/// Characters no file system accepts in names, plus control characters.
pub(crate) fn clean_name(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
            out.push(' ');
        } else {
            out.push(c);
        }
    }
    let collapsed = out.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim_matches(|c: char| c == '.' || c == ' ').to_string();
    let mut name: String = trimmed.chars().take(150).collect();
    let upper = name.to_ascii_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL"].contains(&upper.as_str()) || (upper.len() == 4 && (upper.starts_with("COM") || upper.starts_with("LPT")) && upper.as_bytes()[3].is_ascii_digit());
    if reserved {
        name.push('_');
    }
    if name.is_empty() {
        "Track".into()
    } else {
        name
    }
}

/// "Artist - Title", or the title alone.
pub fn base_name(t: &TrackRow) -> String {
    match t.artist.as_deref().map(str::trim).filter(|a| !a.is_empty()) {
        Some(a) if t.title.as_deref().is_some_and(|x| !x.trim().is_empty()) => clean_name(&format!("{a} - {}", t.display_title())),
        _ => clean_name(&t.display_title()),
    }
}

/// Ranks copies of one recording (best first): score, lossless, resolution,
/// bitrate. Copies that rank the same (identical audio in WAV and AIFF, say)
/// are equally good, and the track the user chose is kept.
fn rank(t: &TrackRow) -> (i64, i64, i64, i64) {
    (
        t.overall.map(|v| (v * 10.0) as i64).unwrap_or(-1),
        t.lossless.map(|l| l as i64).unwrap_or(0),
        t.bit_depth.unwrap_or(0) * 1_000_000 + t.sample_rate.unwrap_or(0),
        // Lossless bitrates only measure compression (FLAC against WAV).
        if t.lossless == Some(true) { 0 } else { t.bitrate_kbps.map(|v| v.round() as i64).unwrap_or(0) },
    )
}

/// Other copies of the same recording: identical files, identical audio, or
/// a fingerprint match along the whole track. Alternate edits do not count.
fn copies(c: &Connection, t: &TrackRow) -> Result<Vec<i64>, StoreError> {
    let mut ids = c.query(
        "SELECT id FROM tracks WHERE id != ? AND ((file_hash IS NOT NULL AND file_hash = ?) OR (audio_hash IS NOT NULL AND audio_hash = ?))",
        params![t.id, t.file_hash.clone(), t.audio_hash.clone()],
        |r| r.i64(0),
    )?;
    ids.extend(c.query(
        "SELECT CASE WHEN a = ? THEN b ELSE a END FROM fp_matches WHERE kind = 'same_recording' AND (a = ? OR b = ?)",
        params![t.id, t.id, t.id],
        |r| r.i64(0),
    )?);
    ids.sort_unstable();
    ids.dedup();
    Ok(ids)
}

fn warnings_for(t: &TrackRow, source: &TrackRow, opts: &ExportOptions) -> Vec<Warning> {
    let mut w = Vec::new();
    let flag = |f: &str| source.flags.iter().any(|x| x == f);
    if source.lossless == Some(false) {
        let rate = source.bitrate_kbps.map(|b| format!(" {} kbps", b.round() as i64)).unwrap_or_default();
        let what = format!("{}{rate}", source.format.as_deref().unwrap_or("lossy"));
        match opts.target {
            Target::Mp3 if source.format.as_deref() == Some("MP3") => {}
            Target::Mp3 => w.push(Warning {
                code: "reencoded",
                message: format!("The source is {what}. Encoding it to MP3 loses a little more quality; AIFF or WAV keeps it as it is."),
            }),
            _ => w.push(Warning {
                code: "lossy_source",
                message: format!("The source is {what}. The new file sounds exactly like it: converting cannot bring back what the lossy encoding removed."),
            }),
        }
    } else if flag("source_concern") || flag("mislabeled") {
        w.push(Warning {
            code: "lossy_origin",
            message: "SONORA found signs that this lossless file was made from a lossy source. The export cannot restore what was lost.".into(),
        });
    }
    if flag("clipping") {
        w.push(Warning { code: "clipping", message: "The source is clipped; the export keeps the clipping.".into() });
    }
    if flag("integrity") {
        w.push(Warning { code: "integrity", message: "The source has integrity problems (damage, truncation or dropouts); check the result.".into() });
    }
    if t.status != "done" {
        w.push(Warning { code: "not_analyzed", message: "Not analyzed yet: the tags carry only the tempo, key and genre the file already had.".into() });
    } else if t.dj_bpm.is_none() || t.dj_key.is_none() {
        w.push(Warning { code: "no_dj_data", message: "No tempo or key known; analyze the track again to detect them.".into() });
    }
    // MP3 is always 44.1 kHz at 320 kbps: the quality choice is for AIFF
    // and WAV, and the dialog does not show it for MP3.
    if opts.target != Target::Mp3 && opts.quality == Quality::Universal && source.lossless == Some(true) && (source.bit_depth.unwrap_or(16) > 16 || source.sample_rate.unwrap_or(44_100) > 48_000) {
        w.push(Warning { code: "downsampled", message: "Reduced to 16-bit/44.1 kHz for compatibility (choose high resolution to keep more).".into() });
    }
    w
}

/// The bit rate of PCM audio, in kbps, rounded (24-bit at 44.1 kHz is
/// 2116.8, so 2117, not 2116).
pub fn pcm_bitrate_kbps(sample_rate: u32, channels: u16, bits: u16) -> u32 {
    ((sample_rate as u64 * channels as u64 * bits as u64 + 500) / 1000).max(1) as u32
}

/// Works out what an export of `ids` would do, in the order given.
pub fn plan(c: &Connection, ids: &[i64], opts: &ExportOptions) -> Result<Vec<PlanItem>, StoreError> {
    let rows = get_tracks(c, ids)?;
    let by_id: HashMap<i64, TrackRow> = rows.into_iter().map(|t| (t.id, t)).collect();
    let mut items = Vec::new();
    let mut used_sources: HashSet<i64> = HashSet::new();
    let mut names: HashSet<String> = HashSet::new();
    for id in ids {
        let Some(t) = by_id.get(id) else { continue };
        let mut source = t.clone();
        if opts.best_copy {
            let others = get_tracks(c, &copies(c, t)?)?;
            for o in others {
                if o.missing_since.is_none() && Path::new(&o.path).exists() && rank(&o) > rank(&source) {
                    source = o;
                }
            }
        }
        let missing = source.missing_since.is_some() || !Path::new(&source.path).exists();
        let mut skip = if missing { Some("missing") } else { None };
        // Two selected copies of one recording are exported once.
        let group: Vec<i64> = std::iter::once(source.id).chain(if opts.best_copy { copies(c, &source)? } else { Vec::new() }).collect();
        if skip.is_none() && group.iter().any(|g| used_sources.contains(g)) {
            skip = Some("duplicate");
        }
        if skip.is_none() {
            used_sources.insert(source.id);
        }
        let base = base_name(t);
        let mut file_name = format!("{base}.{}", opts.target.extension());
        let mut n = 2;
        while names.contains(&file_name.to_lowercase()) {
            file_name = format!("{base} ({n}).{}", opts.target.extension());
            n += 1;
        }
        if skip.is_none() {
            names.insert(file_name.to_lowercase());
        }
        let warnings = if skip.is_some() { Vec::new() } else { warnings_for(t, &source, opts) };
        items.push(PlanItem { track: t.clone(), source, file_name, warnings, skip });
    }
    Ok(items)
}

/// A path in `folder` for `name` that does not exist yet ("Name (2).aiff", …).
pub(crate) fn unique_path(folder: &Path, name: &str) -> PathBuf {
    let p = folder.join(name);
    if !p.exists() {
        return p;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) => (s.to_string(), format!(".{e}")),
        None => (name.to_string(), String::new()),
    };
    (2..).map(|n| folder.join(format!("{stem} ({n}){ext}"))).find(|p| !p.exists()).unwrap_or(p)
}

// ------------------------------------------------------------ rekordbox

pub(crate) fn xml_escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&apos;"),
            // Characters XML 1.0 does not allow at all.
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => {}
            c => o.push(c),
        }
    }
    o
}

/// `file://localhost/…` URL of a path, as Rekordbox writes them.
pub fn file_url(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    let s = if s.starts_with('/') { s } else { format!("/{s}") };
    let mut out = String::from("file://localhost");
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// One exported file, for the XML and the report.
#[derive(Clone, Debug, PartialEq)]
pub struct Written {
    pub track_id: i64,
    pub path: PathBuf,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub genre: Option<String>,
    pub year: Option<String>,
    pub bpm: Option<f64>,
    pub key: Option<String>,
    pub bytes: u64,
    pub duration: f64,
    pub sample_rate: u32,
    pub bitrate_kbps: u32,
    pub method: Method,
    pub warnings: Vec<Warning>,
}

/// A Rekordbox collection with the exported tracks and one playlist of them.
pub fn rekordbox_xml(written: &[Written], playlist: &str, kind: Target, app_version: &str, date: &str) -> String {
    let kind = match kind {
        Target::Aiff => "AIFF File",
        Target::Wav => "WAV File",
        Target::Mp3 => "MP3 File",
    };
    let mut x = String::new();
    x.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<DJ_PLAYLISTS Version=\"1.0.0\">\n");
    x.push_str(&format!("  <PRODUCT Name=\"SONORA\" Version=\"{}\" Company=\"SONORA\"/>\n", xml_escape(app_version)));
    x.push_str(&format!("  <COLLECTION Entries=\"{}\">\n", written.len()));
    for (i, w) in written.iter().enumerate() {
        let attr = |name: &str, v: &str| format!(" {name}=\"{}\"", xml_escape(v));
        let mut t = format!("    <TRACK TrackID=\"{}\"", i + 1);
        t += &attr("Name", &w.title);
        t += &attr("Artist", w.artist.as_deref().unwrap_or(""));
        t += &attr("Album", w.album.as_deref().unwrap_or(""));
        t += &attr("Genre", w.genre.as_deref().unwrap_or(""));
        t += &attr("Kind", kind);
        t += &attr("Size", &w.bytes.to_string());
        t += &attr("TotalTime", &(w.duration.round() as i64).to_string());
        t += &attr("Year", w.year.as_deref().unwrap_or(""));
        t += &attr("AverageBpm", &w.bpm.map(|b| format!("{b:.2}")).unwrap_or_default());
        t += &attr("DateAdded", date);
        t += &attr("BitRate", &w.bitrate_kbps.to_string());
        t += &attr("SampleRate", &w.sample_rate.to_string());
        t += &attr("Tonality", w.key.as_deref().unwrap_or(""));
        t += &attr("Location", &file_url(&w.path));
        t += "/>\n";
        x.push_str(&t);
    }
    x.push_str("  </COLLECTION>\n  <PLAYLISTS>\n    <NODE Type=\"0\" Name=\"ROOT\" Count=\"1\">\n");
    x.push_str(&format!("      <NODE Name=\"{}\" Type=\"1\" KeyType=\"0\" Entries=\"{}\">\n", xml_escape(playlist), written.len()));
    for i in 0..written.len() {
        x.push_str(&format!("        <TRACK Key=\"{}\"/>\n", i + 1));
    }
    x.push_str("      </NODE>\n    </NODE>\n  </PLAYLISTS>\n</DJ_PLAYLISTS>\n");
    x
}

// ------------------------------------------------------------------ job

/// The one export that can run at a time.
#[derive(Default)]
pub struct Exporter {
    running: AtomicBool,
    cancel: Arc<AtomicBool>,
    last: Mutex<Option<Value>>,
}

pub struct Job {
    pub items: Vec<PlanItem>,
    pub folder: PathBuf,
    pub options: ExportOptions,
    pub app_version: String,
    /// Name of the playlist in rekordbox.xml (the interface passes one with
    /// the local date and time; the core only knows UTC).
    pub playlist: Option<String>,
}

/// UTC date and time ("YYYY-MM-DD", "HH:MM").
fn utc_stamp() -> (String, String) {
    // Days since the Unix epoch to a civil date.
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (format!("{y:04}-{m:02}-{d:02}"), format!("{:02}:{:02}", tod / 3600, (tod % 3600) / 60))
}

impl Exporter {
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    pub fn cancel(&self) -> bool {
        if self.is_running() {
            self.cancel.store(true, Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    /// The report of the most recent export.
    pub fn last(&self) -> Value {
        self.last.lock().ok().and_then(|l| l.clone()).unwrap_or(Value::Null)
    }

    /// Starts the export on a background thread. `false` if one is already running.
    pub fn start(self: &Arc<Self>, job: Job, events: Arc<EventBus>, tools: ToolPaths) -> bool {
        if self.running.swap(true, Ordering::SeqCst) {
            return false;
        }
        self.cancel.store(false, Ordering::SeqCst);
        let me = Arc::clone(self);
        let spawned = std::thread::Builder::new().name("sonora-export".into()).spawn(move || {
            let report = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&job, &events, &tools, &me.cancel))).unwrap_or_else(|_| {
                obj! { "error" => "The export stopped because of an internal error. Details are in the log.", "folder" => job.folder.to_string_lossy().to_string() }
            });
            if let Ok(mut l) = me.last.lock() {
                *l = Some(report.clone());
            }
            me.running.store(false, Ordering::SeqCst);
            events.emit("djexport.finished", &report);
        });
        if spawned.is_err() {
            self.running.store(false, Ordering::SeqCst);
            return false;
        }
        true
    }
}

fn run(job: &Job, events: &EventBus, tools: &ToolPaths, cancel: &AtomicBool) -> Value {
    let started = Instant::now();
    let todo: Vec<&PlanItem> = job.items.iter().filter(|i| i.skip.is_none()).collect();
    let total = todo.len();
    let mut written: Vec<Written> = Vec::new();
    let mut failed: Vec<Value> = Vec::new();
    let mut cancelled = false;
    let mut last_emit = Instant::now() - Duration::from_secs(1);
    for (i, item) in todo.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            cancelled = true;
            break;
        }
        let title = item.track.display_title();
        let mut emit = |fraction: f64, force: bool| {
            if force || last_emit.elapsed() >= Duration::from_millis(200) {
                last_emit = Instant::now();
                events.emit(
                    "djexport.progress",
                    &obj! { "done" => i, "total" => total, "fraction" => num((i as f64 + fraction) / total.max(1) as f64, 3), "current" => title.clone() },
                );
            }
        };
        emit(0.0, true);
        match export_one(item, job, tools, cancel, &mut emit) {
            Ok(w) => written.push(w),
            Err(ExportError::Cancelled) => {
                cancelled = true;
                break;
            }
            Err(ExportError::Failed(msg)) => {
                crate::logging::log_fields("warn", "export", &msg, obj! { "track" => item.track.id });
                failed.push(obj! { "trackId" => item.track.id, "title" => title, "error" => msg });
            }
        }
    }
    let mut xml_path: Option<PathBuf> = None;
    let mut xml_error: Option<String> = None;
    if job.options.rekordbox_xml && !written.is_empty() {
        let (date, time) = utc_stamp();
        let playlist = job.playlist.clone().unwrap_or_else(|| format!("SONORA export {date} {time} UTC"));
        let xml = rekordbox_xml(&written, &playlist, job.options.target, &job.app_version, &date);
        let p = unique_path(&job.folder, "rekordbox.xml");
        match std::fs::write(&p, xml) {
            Ok(()) => xml_path = Some(p),
            Err(e) => xml_error = Some(format!("rekordbox.xml could not be written: {e}")),
        }
    }
    let skipped: Vec<Value> = job
        .items
        .iter()
        .filter_map(|i| i.skip.map(|s| obj! { "trackId" => i.track.id, "title" => i.track.display_title(), "reason" => s }))
        .collect();
    obj! {
        "folder" => job.folder.to_string_lossy().to_string(),
        "written" => Value::Array(written.iter().map(|w| obj! {
            "trackId" => w.track_id,
            "path" => w.path.to_string_lossy().to_string(),
            "title" => w.title,
            "method" => match w.method { Method::Native => "native", Method::Converted => "converted", Method::Encoded => "encoded", Method::Copied => "copied" },
            "bytes" => w.bytes as i64,
            "warnings" => Value::Array(w.warnings.iter().map(ToJson::to_json).collect()),
        }).collect()),
        "failed" => Value::Array(failed),
        "skipped" => Value::Array(skipped),
        "cancelled" => cancelled,
        "rekordboxXml" => xml_path.map(|p| p.to_string_lossy().to_string()),
        "xmlError" => xml_error,
        "elapsedMs" => started.elapsed().as_millis() as i64,
    }
}

pub(crate) enum ExportError {
    Cancelled,
    Failed(String),
}

fn export_one(item: &PlanItem, job: &Job, tools: &ToolPaths, cancel: &AtomicBool, emit: &mut dyn FnMut(f64, bool)) -> Result<Written, ExportError> {
    export_file(item, &job.folder, &item.file_name, &job.options, tools, cancel, emit)
}

/// Writes one DJ-ready file for `item` into `folder`, as `file_name` or,
/// when that name is taken, "name (2)" and so on: nothing is overwritten.
pub(crate) fn export_file(
    item: &PlanItem,
    folder: &Path,
    file_name: &str,
    options: &ExportOptions,
    tools: &ToolPaths,
    cancel: &AtomicBool,
    emit: &mut dyn FnMut(f64, bool),
) -> Result<Written, ExportError> {
    let src = Path::new(&item.source.path);
    let facts = probe_file(src, tools).map_err(|e| ExportError::Failed(format!("the source cannot be read: {e}")))?;
    let t = &item.track;
    let key = t.dj_key.filter(|k| (0..24).contains(k)).map(|k| format_key(k as u8, options.notation, true));
    let tags = TagSet {
        title: t.title.clone().or_else(|| facts.tags.title.clone()).or_else(|| Some(t.display_title())),
        artist: t.artist.clone().or_else(|| facts.tags.artist.clone()),
        album: t.album.clone().or_else(|| facts.tags.album.clone()),
        genre: t.dj_genre.clone().or_else(|| facts.tags.genre.clone()),
        year: facts.tags.date.clone(),
        bpm: t.dj_bpm,
        key: key.clone(),
        comment: facts.tags.comment.clone(),
        picture: picture::extract(src, facts.container, tools),
    };
    let dest = unique_path(folder, file_name);
    let final_name = dest.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let tmp = folder.join(format!(".{final_name}.sonora-part"));
    let req = ConvertRequest { src, facts: &facts, target: options.target, quality: options.quality, tags: &tags, dest: &tmp, tools, cancel };
    let result = convert(&req, &mut |p| emit(p, false));
    let out = match result {
        Ok(o) => o,
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(match e {
                sonora_engine::AnalysisError::Cancelled => ExportError::Cancelled,
                other => ExportError::Failed(other.to_string()),
            });
        }
    };
    // Another file may have taken the name meanwhile: never replace it.
    let dest = if dest.exists() { unique_path(folder, file_name) } else { dest };
    if let Err(e) = std::fs::rename(&tmp, &dest) {
        let _ = std::fs::remove_file(&tmp);
        return Err(ExportError::Failed(format!("the file could not be saved as {}: {e}", dest.display())));
    }
    let mut warnings = item.warnings.clone();
    if out.clipped_samples > 0 {
        warnings.push(Warning { code: "overs", message: format!("{} samples above full scale were limited to it.", out.clipped_samples) });
    }
    if out.decode_errors > 0 {
        warnings.push(Warning { code: "decode_errors", message: format!("The decoder reported {} errors in the source.", out.decode_errors) });
    }
    let bitrate = match out.bits {
        Some(b) => pcm_bitrate_kbps(out.sample_rate, out.channels, b),
        None => {
            if out.method == Method::Copied {
                facts.bitrate_kbps.map(|b| b.round() as u32).unwrap_or(0)
            } else {
                320
            }
        }
    };
    // An MP3 copied frame for frame has no header to say how long it is;
    // the library knows from the analysis.
    let duration = if out.duration > 0.0 { out.duration } else { item.source.duration.unwrap_or(0.0) };
    Ok(Written {
        track_id: t.id,
        path: dest,
        title: tags.title.clone().unwrap_or_else(|| t.display_title()),
        artist: tags.artist.clone(),
        album: tags.album.clone(),
        genre: tags.genre.clone(),
        year: tags.year.as_deref().and_then(sonora_engine::export::id3::year_of).map(str::to_string),
        bpm: tags.bpm,
        key,
        bytes: out.bytes,
        duration,
        sample_rate: out.sample_rate,
        bitrate_kbps: bitrate,
        method: out.method,
        warnings,
    })
}

/// Whether this export can run with the tools at hand.
pub fn check_tools(opts: &ExportOptions, items: &[PlanItem], tools: &ToolPaths) -> Result<(), String> {
    if opts.target == Target::Mp3 && items.iter().any(|i| i.skip.is_none() && i.source.format.as_deref() != Some("MP3")) && !has_mp3_encoder(tools) {
        return Err(if tools.ffmpeg.is_none() {
            "FFmpeg is needed to encode MP3 files but was not found.".into()
        } else {
            "This FFmpeg cannot encode MP3 files (it was built without LAME). Choose AIFF or WAV instead.".into()
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_safe_everywhere() {
        assert_eq!(clean_name("AC/DC: Back in Black?"), "AC DC Back in Black");
        assert_eq!(clean_name("  ...dots...  "), "dots");
        assert_eq!(clean_name("con"), "con_");
        assert_eq!(clean_name("LPT1"), "LPT1_");
        assert_eq!(clean_name("\u{0}\u{7}"), "Track");
        assert_eq!(clean_name(&"x".repeat(400)).len(), 150);
        assert_eq!(clean_name("Ñandú – Café"), "Ñandú – Café");
    }

    #[test]
    fn urls_and_xml() {
        assert_eq!(file_url(Path::new("/Users/dj/Music/SONORA Export/A&B #1.aiff")), "file://localhost/Users/dj/Music/SONORA%20Export/A%26B%20%231.aiff");
        assert_eq!(file_url(Path::new(r"C:\Music\Ñ.aiff")), "file://localhost/C:/Music/%C3%91.aiff");
        let w = Written {
            track_id: 1,
            path: PathBuf::from("/m/a.aiff"),
            title: "Rock & \"Roll\" <live>".into(),
            artist: Some("O'Neil".into()),
            album: None,
            genre: Some("House".into()),
            year: Some("2024".into()),
            bpm: Some(124.0),
            key: Some("8A".into()),
            bytes: 1000,
            duration: 61.6,
            sample_rate: 44_100,
            bitrate_kbps: 1411,
            method: Method::Native,
            warnings: Vec::new(),
        };
        let x = rekordbox_xml(&[w], "SONORA export", Target::Aiff, "1.0", "2026-10-19");
        assert!(x.contains(r#"Name="Rock &amp; &quot;Roll&quot; &lt;live&gt;""#), "{x}");
        assert!(x.contains(r#"Artist="O&apos;Neil""#));
        assert!(x.contains(r#"AverageBpm="124.00""#));
        assert!(x.contains(r#"TotalTime="62""#));
        assert!(x.contains(r#"Tonality="8A""#));
        assert!(x.contains(r#"Kind="AIFF File""#));
        assert!(x.contains(r#"<TRACK Key="1"/>"#));
        assert!(x.starts_with("<?xml"));
    }

    fn row(lossless: bool, bits: i64, rate: i64, duration: f64) -> TrackRow {
        TrackRow {
            id: 1,
            path: "/m/a.wav".into(),
            status: "done".into(),
            lossless: Some(lossless),
            bit_depth: Some(bits),
            sample_rate: Some(rate),
            duration: Some(duration),
            dj_bpm: Some(124.0),
            dj_key: Some(8),
            ..TrackRow::default()
        }
    }

    #[test]
    fn mp3_exports_carry_no_resolution_warning_and_pcm_bit_rates_are_rounded() {
        let hires = row(true, 24, 96_000, 300.0);
        let opts = |target: Target, quality: Quality| ExportOptions { target, quality, notation: Notation::Camelot, best_copy: true, rekordbox_xml: true };
        let codes = |o: ExportOptions| warnings_for(&hires, &hires, &o).iter().map(|w| w.code).collect::<Vec<_>>();
        assert_eq!(codes(opts(Target::Aiff, Quality::Universal)), ["downsampled"]);
        assert_eq!(codes(opts(Target::Wav, Quality::Universal)), ["downsampled"]);
        assert!(codes(opts(Target::Aiff, Quality::High)).is_empty());
        // MP3 has no quality choice: what the dialog last showed for AIFF must not change the plan.
        assert!(codes(opts(Target::Mp3, Quality::Universal)).is_empty(), "no advice to choose a resolution that MP3 does not offer");
        assert!(codes(opts(Target::Mp3, Quality::High)).is_empty());
        assert_eq!(pcm_bitrate_kbps(44_100, 2, 16), 1411);
        assert_eq!(pcm_bitrate_kbps(44_100, 2, 24), 2117, "2116.8 kbps, not 2116");
        assert_eq!(pcm_bitrate_kbps(48_000, 2, 24), 2304);
        assert_eq!(pcm_bitrate_kbps(44_100, 1, 16), 706);
    }

    #[test]
    fn dates() {
        let (d, t) = utc_stamp();
        assert_eq!(d.len(), 10);
        assert!(d.starts_with("20"));
        assert_eq!(t.len(), 5);
    }
}
