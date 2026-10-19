//! Library operations: importing paths, change/move/missing detection,
//! queries for the interface, statistics, and storing analysis results.

use crate::db::sqlite::{Connection, SqlValue, ToSql};
use crate::db::{now_ms, Db, StoreError};
use crate::dj::DjConfig;
use crate::index::{self, Candidate, WalkOptions};
use crate::model::{flags_to_column, format_label, status, TrackRow, TRACK_COLUMNS};
use crate::params;
use sonora_engine::FileAnalysis;
use sonora_json::{num, obj, ToJson, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

// ------------------------------------------------------------------ import

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImportReport {
    /// Tracks that need analysis (new, changed or never finished).
    pub queue: Vec<i64>,
    pub added: u64,
    pub changed: u64,
    pub unchanged: u64,
    pub relinked: u64,
    pub restored: u64,
    pub marked_missing: u64,
    pub folders: Vec<i64>,
    pub roots: Vec<String>,
    pub errors: Vec<String>,
}

impl ToJson for ImportReport {
    fn to_json(&self) -> Value {
        obj! {
            "queued" => self.queue.len(),
            "added" => self.added,
            "changed" => self.changed,
            "unchanged" => self.unchanged,
            "relinked" => self.relinked,
            "restored" => self.restored,
            "markedMissing" => self.marked_missing,
            "folders" => self.folders,
            "errors" => self.errors,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ImportOptions {
    pub walk: WalkOptions,
    /// Re-analyse files whose size or mtime changed.
    pub reanalyze_changed: bool,
    /// Re-analyse everything found, even if unchanged.
    pub force: bool,
}

fn escape_like(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for ch in s.chars() {
        if ch == '%' || ch == '_' || ch == '\\' {
            o.push('\\');
        }
        o.push(ch);
    }
    o
}

fn path_str(p: &Path) -> String {
    p.to_string_lossy().to_string()
}

fn ext_of(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

fn file_name_of(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
}

/// Registers a folder (idempotent); returns its id.
pub fn upsert_folder(c: &Connection, path: &Path) -> Result<i64, StoreError> {
    let p = path_str(path);
    if let Some(id) = c.query_row("SELECT id FROM folders WHERE path = ?", params![p], |r| r.i64(0))? {
        c.execute("UPDATE folders SET available = 1 WHERE id = ?", params![id])?;
        return Ok(id);
    }
    c.execute("INSERT INTO folders (path, added_at) VALUES (?, ?)", params![p, now_ms()])?;
    Ok(c.last_insert_rowid())
}

/// The registered folder containing `path`, if any (deepest match).
pub fn folder_for(c: &Connection, path: &Path) -> Result<Option<i64>, StoreError> {
    let folders = c.query("SELECT id, path FROM folders", params![], |r| (r.i64(0), r.text(1)))?;
    Ok(folders.into_iter().filter(|(_, fp)| index::is_within(path, Path::new(fp))).max_by_key(|(_, fp)| fp.len()).map(|(id, _)| id))
}

#[derive(Debug, PartialEq)]
enum Outcome {
    New(i64),
    Changed(i64),
    Unchanged(i64),
    Pending(i64),
    Relinked(i64),
}

fn index_candidate(c: &Connection, cand: &Candidate, folder_id: Option<i64>, opts: &ImportOptions) -> Result<(Outcome, bool), StoreError> {
    let p = path_str(&cand.path);
    let now = now_ms();
    let existing = c.query_row("SELECT id, size, mtime, status, missing_since FROM tracks WHERE path = ?", params![p], |r| {
        (r.i64(0), r.i64(1), r.i64(2), r.text(3), r.opt_i64(4))
    })?;
    if let Some((id, size, mtime, st, missing)) = existing {
        let restored = missing.is_some();
        if restored {
            c.execute("UPDATE tracks SET missing_since = NULL WHERE id = ?", params![id])?;
        }
        if folder_id.is_some() {
            c.execute("UPDATE tracks SET folder_id = COALESCE(folder_id, ?) WHERE id = ?", params![folder_id, id])?;
        }
        let same = size == cand.size as i64 && mtime == cand.mtime;
        if !same {
            let qh = index::quick_hash(&cand.path).map_err(|e| StoreError::Io(e.to_string()))?;
            c.execute("UPDATE tracks SET size = ?, mtime = ?, quick_hash = ? WHERE id = ?", params![cand.size as i64, cand.mtime, qh, id])?;
            if opts.reanalyze_changed || st != status::DONE {
                return Ok((Outcome::Changed(id), restored));
            }
            return Ok((Outcome::Unchanged(id), restored));
        }
        if opts.force {
            return Ok((Outcome::Changed(id), restored));
        }
        return Ok((if st == status::DONE || st == status::FAILED { Outcome::Unchanged(id) } else { Outcome::Pending(id) }, restored));
    }
    let qh = index::quick_hash(&cand.path).map_err(|e| StoreError::Io(e.to_string()))?;
    // A file with the same identity whose old path is gone: it was moved or renamed.
    let same_identity = c.query("SELECT id, path FROM tracks WHERE quick_hash = ? AND size = ?", params![qh, cand.size as i64], |r| (r.i64(0), r.text(1)))?;
    for (id, old_path) in same_identity {
        if !Path::new(&old_path).exists() {
            c.execute(
                "UPDATE tracks SET path = ?, file_name = ?, ext = ?, folder_id = ?, mtime = ?, missing_since = NULL WHERE id = ?",
                params![p, file_name_of(&cand.path), ext_of(&cand.path), folder_id, cand.mtime, id],
            )?;
            return Ok((Outcome::Relinked(id), false));
        }
    }
    let name = file_name_of(&cand.path);
    c.execute(
        "INSERT INTO tracks (path, folder_id, file_name, ext, size, mtime, quick_hash, status, added_at, sort_title) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        params![p, folder_id, name, ext_of(&cand.path), cand.size as i64, cand.mtime, qh, status::PENDING, now, name.to_lowercase()],
    )?;
    Ok((Outcome::New(c.last_insert_rowid()), false))
}

fn record(report: &mut ImportReport, o: Outcome, restored: bool) {
    if restored {
        report.restored += 1;
    }
    match o {
        Outcome::New(id) => {
            report.added += 1;
            report.queue.push(id);
        }
        Outcome::Changed(id) => {
            report.changed += 1;
            report.queue.push(id);
        }
        Outcome::Pending(id) => report.queue.push(id),
        Outcome::Unchanged(_) => report.unchanged += 1,
        Outcome::Relinked(_) => report.relinked += 1,
    }
}

/// Marks tracks under `root` that were not seen during the walk and no
/// longer exist on disk as missing (their results are kept).
fn mark_missing_under(c: &Connection, root: &Path, seen: &HashSet<String>) -> Result<u64, StoreError> {
    let pattern = format!("{}%", under_pattern(root));
    let rows = c.query("SELECT id, path FROM tracks WHERE missing_since IS NULL AND path LIKE ? ESCAPE '\\'", params![pattern], |r| (r.i64(0), r.text(1)))?;
    let now = now_ms();
    let mut n = 0;
    for (id, p) in rows {
        if !seen.contains(&p) && !Path::new(&p).exists() {
            c.execute("UPDATE tracks SET missing_since = ? WHERE id = ?", params![now, id])?;
            n += 1;
        }
    }
    Ok(n)
}

/// LIKE prefix (escaped for `ESCAPE '\\'`) of the paths inside `root`: the
/// root and the platform's separator. The separator is escaped too: on
/// Windows it is the backslash, which would otherwise escape what follows.
fn under_pattern(root: &Path) -> String {
    like_prefix(&path_str(root), std::path::MAIN_SEPARATOR)
}

fn like_prefix(root: &str, sep: char) -> String {
    escape_like(&format!("{}{}", root.trim_end_matches(['/', '\\']), sep))
}

/// Indexes `paths` (files and/or folders). Folders are registered as
/// library folders and walked recursively.
pub fn import_paths(db: &Db, paths: &[PathBuf], opts: &ImportOptions, cancel: &AtomicBool) -> Result<ImportReport, StoreError> {
    let mut report = ImportReport::default();
    for root in paths {
        if root.is_dir() {
            let folder_id = {
                let c = db.write();
                upsert_folder(&c, root)?
            };
            report.folders.push(folder_id);
            report.roots.push(path_str(root));
            let mut seen: HashSet<String> = HashSet::new();
            let mut batch: Vec<Candidate> = Vec::new();
            let flush = |batch: &mut Vec<Candidate>, report: &mut ImportReport| -> Result<(), StoreError> {
                if batch.is_empty() {
                    return Ok(());
                }
                let c = db.write();
                c.transaction(|tx| {
                    for cand in batch.iter() {
                        match index_candidate(tx, cand, Some(folder_id), opts) {
                            Ok((o, restored)) => record(report, o, restored),
                            Err(StoreError::Io(e)) => report.errors.push(format!("{}: {e}", cand.path.display())),
                            Err(e) => return Err(e),
                        }
                    }
                    Ok::<(), StoreError>(())
                })?;
                batch.clear();
                Ok(())
            };
            let mut walk_err: Option<StoreError> = None;
            let stats = index::walk(root, opts.walk, cancel, |cand| {
                if walk_err.is_some() {
                    return;
                }
                seen.insert(path_str(&cand.path));
                batch.push(cand);
                if batch.len() >= 200 {
                    if let Err(e) = flush(&mut batch, &mut report) {
                        walk_err = Some(e);
                    }
                }
            });
            if let Some(e) = walk_err {
                return Err(e);
            }
            flush(&mut batch, &mut report)?;
            report.errors.extend(stats.unreadable.iter().cloned());
            if !cancel.load(std::sync::atomic::Ordering::Relaxed) {
                let c = db.write();
                report.marked_missing += mark_missing_under(&c, root, &seen)?;
                c.execute("UPDATE folders SET last_scan_at = ?, available = 1 WHERE id = ?", params![now_ms(), folder_id])?;
            }
        } else if root.is_file() {
            // Explicitly chosen files are accepted whatever their extension;
            // the analysis decides from the content whether they are audio.
            let cand = index::file_candidate(root).map_err(|e| StoreError::Io(e.to_string()))?;
            let c = db.write();
            let folder_id = folder_for(&c, root)?;
            match index_candidate(&c, &cand, folder_id, opts) {
                Ok((o, restored)) => record(&mut report, o, restored),
                Err(StoreError::Io(e)) => report.errors.push(format!("{}: {e}", root.display())),
                Err(e) => return Err(e),
            }
            report.roots.push(path_str(root));
        } else {
            report.errors.push(format!("{}: not found", root.display()));
        }
    }
    report.queue.sort_unstable();
    report.queue.dedup();
    Ok(report)
}

/// Re-walks registered folders (all, or the given ids).
pub fn rescan_folders(db: &Db, ids: Option<&[i64]>, opts: &ImportOptions, cancel: &AtomicBool) -> Result<ImportReport, StoreError> {
    let folders: Vec<(i64, String)> = {
        let c = db.read();
        c.query("SELECT id, path FROM folders ORDER BY path", params![], |r| (r.i64(0), r.text(1)))?
    };
    let mut paths = Vec::new();
    let mut unavailable = Vec::new();
    for (id, p) in folders {
        if ids.is_none_or(|ids| ids.contains(&id)) {
            if Path::new(&p).is_dir() {
                paths.push(PathBuf::from(p));
            } else {
                unavailable.push((id, p));
            }
        }
    }
    let mut report = import_paths(db, &paths, opts, cancel)?;
    if !unavailable.is_empty() {
        let c = db.write();
        for (id, p) in unavailable {
            c.execute("UPDATE folders SET available = 0 WHERE id = ?", params![id])?;
            report.errors.push(format!("{p}: folder is not available (disconnected drive?)"));
        }
    }
    Ok(report)
}

// ----------------------------------------------------------------- results

/// Stores a completed analysis atomically, and refreshes the track's DJ values.
pub fn store_analysis(c: &Connection, track_id: i64, a: &FileAnalysis, keep_visuals: bool, dj: &DjConfig) -> Result<(), StoreError> {
    let f = &a.facts;
    let m = &a.signal.measurements;
    let s = &a.assessment;
    let cutoff = m.bandwidth.edge.as_ref().filter(|e| e.is_steep()).map(|e| e.hz);
    let title = f.tags.title.clone();
    let file_name = c.query_row("SELECT file_name FROM tracks WHERE id = ?", params![track_id], |r| r.text(0))?.unwrap_or_default();
    let sort_title = title.clone().unwrap_or(file_name).to_lowercase();
    let lossless: Option<i64> = f.lossless.map(|b| b as i64);
    let bit_depth: Option<i64> = f.bit_depth.map(|b| b as i64);
    let format = format_label(&f.codec_id, &f.container_name);
    let facts_json = f.to_json().to_string_compact();
    let meas_json = m.to_json().to_string_compact();
    let assess_json = s.to_json().to_string_compact();
    let visuals_json = if keep_visuals { Some(a.signal.visuals.to_json().to_string_compact()) } else { None };
    let fp = sonora_engine::analysis::fingerprint::to_bytes(&a.signal.fingerprint);
    let tempo = m.tempo.as_ref();
    let key = m.key.as_ref();
    c.transaction(|tx| {
        tx.execute(
            "UPDATE tracks SET status = ?, error_code = NULL, error_message = NULL, analyzed_at = ?, file_hash = ?, audio_hash = ?, \
             title = ?, artist = ?, album = ?, genre = ?, bpm = ?, musical_key = ?, container = ?, codec = ?, codec_name = ?, format = ?, \
             lossless = ?, sample_rate = ?, bit_depth = ?, bitrate_kbps = ?, channels = ?, duration = ?, integrated_lufs = ?, true_peak = ?, \
             cutoff_hz = ?, upper_hz = ?, overall = ?, dj = ?, listening = ?, confidence = ?, confidence_label = ?, verdict = ?, flags = ?, \
             model_version = ?, engine_version = ?, sort_title = ? WHERE id = ?",
            params![
                status::DONE,
                now_ms(),
                a.file_hash,
                a.signal.audio_hash,
                title,
                f.tags.artist,
                f.tags.album,
                f.tags.genre,
                f.tags.bpm,
                f.tags.key,
                f.container_name,
                f.codec_id,
                f.codec_name,
                format,
                lossless,
                f.sample_rate,
                bit_depth,
                f.bitrate_kbps,
                f.channels,
                m.duration,
                m.integrated_lufs,
                m.true_peak_dbtp,
                cutoff,
                m.bandwidth.upper_hz,
                s.scores.overall,
                s.scores.dj,
                s.scores.listening,
                s.confidence,
                s.confidence_label,
                s.verdict,
                flags_to_column(&s.flags),
                s.model_version,
                m.engine_version,
                sort_title,
                track_id
            ],
        )?;
        tx.execute(
            "UPDATE tracks SET dj_version = 1, bpm_detected = ?, bpm_confidence = ?, tempo_variable = ?, tempo_candidates = ?, key_detected = ?, \
             key_confidence = ?, key_second = ? WHERE id = ?",
            params![
                tempo.map(|t| t.bpm),
                tempo.map(|t| t.confidence),
                tempo.map(|t| t.variable as i64),
                tempo.map(|t| crate::dj::candidates_to_column(&t.candidates)),
                key.map(|k| k.key as i64),
                key.map(|k| k.confidence),
                key.map(|k| k.second as i64),
                track_id
            ],
        )?;
        crate::dj::refresh_within(tx, Some(&[track_id]), dj)?;
        tx.execute(
            "INSERT INTO analyses (track_id, facts, measurements, assessment, visuals, fingerprint, created_at) VALUES (?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(track_id) DO UPDATE SET facts = excluded.facts, measurements = excluded.measurements, assessment = excluded.assessment, \
             visuals = excluded.visuals, fingerprint = excluded.fingerprint, created_at = excluded.created_at",
            params![track_id, facts_json, meas_json, assess_json, visuals_json, fp, now_ms()],
        )?;
        // Matches against an old version of this file are stale.
        tx.execute("DELETE FROM fp_matches WHERE a = ? OR b = ?", params![track_id, track_id])?;
        Ok::<(), StoreError>(())
    })
}

/// Re-scores analyses made with another scoring-model version from their
/// stored facts and measurements, without decoding any audio. Analyses whose
/// stored data cannot be read by this version keep their scores until they
/// are analyzed again. Returns the number of tracks updated.
pub fn rescore_outdated(db: &Db, cfg: &sonora_engine::assess::ScoringConfig) -> Result<u64, StoreError> {
    use sonora_engine::analysis::measure::Measurements;
    use sonora_engine::meta::FileFacts;
    let current = sonora_engine::assess::SCORE_MODEL_VERSION;
    let ids: Vec<i64> = db.read().query(
        "SELECT id FROM tracks WHERE status = 'done' AND (model_version IS NULL OR model_version != ?) ORDER BY id",
        params![current],
        |r| r.i64(0),
    )?;
    let mut updated = 0u64;
    for chunk in ids.chunks(200) {
        let stored: Vec<(i64, String, String)> = {
            let c = db.read();
            let mut v = Vec::with_capacity(chunk.len());
            for &id in chunk {
                if let Some((f, m)) = c.query_row("SELECT facts, measurements FROM analyses WHERE track_id = ?", params![id], |r| (r.text(0), r.text(1)))? {
                    v.push((id, f, m));
                }
            }
            v
        };
        let c = db.write();
        c.transaction(|tx| {
            for (id, f, m) in &stored {
                let facts = sonora_json::parse(f).ok().and_then(|v| FileFacts::from_json(&v));
                let meas = sonora_json::parse(m).ok().and_then(|v| Measurements::from_json(&v));
                let (Some(facts), Some(meas)) = (facts, meas) else { continue };
                let s = sonora_engine::assess::assess(&facts, &meas, cfg);
                tx.execute(
                    "UPDATE tracks SET overall = ?, dj = ?, listening = ?, confidence = ?, confidence_label = ?, verdict = ?, flags = ?, model_version = ? WHERE id = ?",
                    params![s.scores.overall, s.scores.dj, s.scores.listening, s.confidence, s.confidence_label, s.verdict, flags_to_column(&s.flags), s.model_version, *id],
                )?;
                tx.execute("UPDATE analyses SET assessment = ? WHERE track_id = ?", params![s.to_json().to_string_compact(), *id])?;
                updated += 1;
            }
            Ok::<(), StoreError>(())
        })?;
    }
    Ok(updated)
}

pub fn store_failure(c: &Connection, track_id: i64, code: &str, message: &str) -> Result<(), StoreError> {
    c.execute(
        "UPDATE tracks SET status = ?, error_code = ?, error_message = ?, analyzed_at = ? WHERE id = ?",
        params![status::FAILED, code, message, now_ms(), track_id],
    )?;
    Ok(())
}

// ----------------------------------------------------------------- queries

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Filter {
    pub search: Option<String>,
    /// "overall" | "dj" | "listening": which score min/max apply to.
    pub score_kind: String,
    pub min_score: Option<f64>,
    pub max_score: Option<f64>,
    /// Display formats ("FLAC", "MP3"…) or the groups "lossless" / "lossy".
    pub formats: Vec<String>,
    /// Tracks having any of these flags.
    pub flags: Vec<String>,
    /// "done" | "pending" | "analyzing" | "failed" | "missing".
    pub statuses: Vec<String>,
    pub folder_id: Option<i64>,
    pub attention: bool,
    /// Tempo range (inclusive), on the tempo in use.
    pub bpm_min: Option<f64>,
    pub bpm_max: Option<f64>,
    /// Also match half and double the tempo range (mixing at another metre).
    pub bpm_half_double: bool,
    /// Keys (0–23) in use.
    pub keys: Vec<u8>,
    /// Genres in use (case-insensitive exact match).
    pub genres: Vec<String>,
}

impl Filter {
    pub fn from_json(v: Option<&Value>) -> Result<Filter, String> {
        let mut f = Filter { score_kind: "overall".into(), ..Default::default() };
        let Some(v) = v.filter(|v| !v.is_null()) else { return Ok(f) };
        if v.as_object().is_none() {
            return Err("filter must be an object".into());
        }
        let strings = |key: &str| -> Result<Vec<String>, String> {
            match v.get(key) {
                None | Some(Value::Null) => Ok(Vec::new()),
                Some(Value::Array(a)) => a.iter().map(|x| x.as_str().map(str::to_string).ok_or(format!("{key} must contain strings"))).collect(),
                Some(_) => Err(format!("{key} must be an array")),
            }
        };
        f.search = v.get("search").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
        if let Some(k) = v.get("scoreKind").and_then(Value::as_str) {
            if !["overall", "dj", "listening"].contains(&k) {
                return Err("scoreKind must be overall, dj or listening".into());
            }
            f.score_kind = k.into();
        }
        f.min_score = v.get("minScore").and_then(Value::as_f64);
        f.max_score = v.get("maxScore").and_then(Value::as_f64);
        f.formats = strings("formats")?;
        f.flags = strings("flags")?;
        f.statuses = strings("statuses")?;
        f.folder_id = v.get("folderId").and_then(Value::as_i64);
        f.attention = v.get("attention").and_then(Value::as_bool).unwrap_or(false);
        f.bpm_min = v.get("bpmMin").and_then(Value::as_f64);
        f.bpm_max = v.get("bpmMax").and_then(Value::as_f64);
        f.bpm_half_double = v.get("bpmHalfDouble").and_then(Value::as_bool).unwrap_or(false);
        f.keys = match v.get("keys") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(a)) => a
                .iter()
                .map(|k| k.as_i64().filter(|k| (0..24).contains(k)).map(|k| k as u8).ok_or("keys must contain numbers 0–23".to_string()))
                .collect::<Result<_, _>>()?,
            Some(_) => return Err("keys must be an array".into()),
        };
        f.genres = strings("genres")?;
        Ok(f)
    }

    /// SQL `WHERE` clause and its parameters.
    pub fn sql(&self) -> (String, Vec<SqlValue>) {
        let mut w: Vec<String> = Vec::new();
        let mut p: Vec<SqlValue> = Vec::new();
        if let Some(s) = &self.search {
            for tok in s.split_whitespace().take(8) {
                let like = format!("%{}%", escape_like(tok));
                w.push(
                    "(title LIKE ? ESCAPE '\\' OR artist LIKE ? ESCAPE '\\' OR album LIKE ? ESCAPE '\\' OR file_name LIKE ? ESCAPE '\\' OR genre LIKE ? ESCAPE '\\' \
                     OR dj_genre LIKE ? ESCAPE '\\')"
                        .into(),
                );
                for _ in 0..6 {
                    p.push(SqlValue::Text(like.clone()));
                }
            }
        }
        let col = match self.score_kind.as_str() {
            "dj" => "dj",
            "listening" => "listening",
            _ => "overall",
        };
        if let Some(min) = self.min_score {
            w.push(format!("{col} >= ?"));
            p.push(SqlValue::Real(min));
        }
        if let Some(max) = self.max_score {
            w.push(format!("{col} <= ?"));
            p.push(SqlValue::Real(max));
        }
        if !self.formats.is_empty() {
            let mut parts = Vec::new();
            for f in &self.formats {
                match f.as_str() {
                    "lossless" => parts.push("lossless = 1".to_string()),
                    "lossy" => parts.push("lossless = 0".to_string()),
                    other => {
                        parts.push("format = ?".to_string());
                        p.push(SqlValue::Text(other.to_string()));
                    }
                }
            }
            w.push(format!("({})", parts.join(" OR ")));
        }
        if !self.flags.is_empty() {
            let parts: Vec<String> = self.flags.iter().map(|_| "flags LIKE ? ESCAPE '\\'".to_string()).collect();
            for fl in &self.flags {
                p.push(SqlValue::Text(format!("%,{},%", escape_like(fl))));
            }
            w.push(format!("({})", parts.join(" OR ")));
        }
        if !self.statuses.is_empty() {
            let mut parts = Vec::new();
            for s in &self.statuses {
                if s == "missing" {
                    parts.push("missing_since IS NOT NULL".to_string());
                } else {
                    parts.push("status = ?".to_string());
                    p.push(SqlValue::Text(s.clone()));
                }
            }
            w.push(format!("({})", parts.join(" OR ")));
        }
        if let Some(fid) = self.folder_id {
            w.push("folder_id = ?".into());
            p.push(SqlValue::Int(fid));
        }
        if self.attention {
            w.push("(flags LIKE '%,attention,%' OR flags LIKE '%,review,%' OR status = 'failed' OR missing_since IS NOT NULL)".into());
        }
        if self.bpm_min.is_some() || self.bpm_max.is_some() {
            let lo = self.bpm_min.unwrap_or(0.0);
            let hi = self.bpm_max.unwrap_or(10_000.0);
            if self.bpm_half_double {
                w.push("(dj_bpm BETWEEN ? AND ? OR dj_bpm BETWEEN ? AND ? OR dj_bpm BETWEEN ? AND ?)".into());
                for (a, b) in [(lo, hi), (lo / 2.0, hi / 2.0), (lo * 2.0, hi * 2.0)] {
                    p.push(SqlValue::Real(a));
                    p.push(SqlValue::Real(b));
                }
            } else {
                w.push("dj_bpm BETWEEN ? AND ?".into());
                p.push(SqlValue::Real(lo));
                p.push(SqlValue::Real(hi));
            }
        }
        if !self.keys.is_empty() {
            w.push(format!("dj_key IN ({})", vec!["?"; self.keys.len()].join(",")));
            for &k in &self.keys {
                p.push(SqlValue::Int(k as i64));
            }
        }
        if !self.genres.is_empty() {
            w.push(format!("({})", vec!["dj_genre = ? COLLATE NOCASE"; self.genres.len()].join(" OR ")));
            for g in &self.genres {
                p.push(SqlValue::Text(g.clone()));
            }
        }
        if w.is_empty() {
            ("1 = 1".into(), p)
        } else {
            (w.join(" AND "), p)
        }
    }
}

/// Sort key → SQL expression (a whitelist; user input never reaches SQL text).
pub fn sort_expr(key: &str) -> Option<&'static str> {
    Some(match key {
        "title" => "sort_title",
        "artist" => "lower(artist)",
        "album" => "lower(album)",
        "duration" => "duration",
        "format" => "format",
        "sampleRate" => "sample_rate",
        "bitDepth" => "bit_depth",
        "bitrate" => "bitrate_kbps",
        "overall" => "overall",
        "dj" => "dj",
        "listening" => "listening",
        "confidence" => "confidence",
        "lufs" => "integrated_lufs",
        "truePeak" => "true_peak",
        "cutoff" => "cutoff_hz",
        "size" => "size",
        "added" => "added_at",
        "analyzed" => "analyzed_at",
        "path" => "path",
        "status" => "status",
        "bpm" => "dj_bpm",
        "key" => "dj_key_order",
        "genre" => "lower(dj_genre)",
        _ => return None,
    })
}

pub fn as_params(v: &[SqlValue]) -> Vec<&dyn ToSql> {
    v.iter().map(|x| x as &dyn ToSql).collect()
}

pub fn query_tracks(c: &Connection, filter: &Filter, sort_key: &str, desc: bool, offset: i64, limit: i64) -> Result<(i64, Vec<TrackRow>), StoreError> {
    let (w, p) = filter.sql();
    let total = c.query_row(&format!("SELECT COUNT(*) FROM tracks WHERE {w}"), &as_params(&p), |r| r.i64(0))?.unwrap_or(0);
    let expr = sort_expr(sort_key).unwrap_or("sort_title");
    let dir = if desc { "DESC" } else { "ASC" };
    let sql = format!("SELECT {TRACK_COLUMNS} FROM tracks WHERE {w} ORDER BY ({expr}) IS NULL, {expr} {dir}, id {dir} LIMIT ? OFFSET ?");
    let mut params_v = p.clone();
    params_v.push(SqlValue::Int(limit.clamp(1, 2000)));
    params_v.push(SqlValue::Int(offset.max(0)));
    let rows = c.query(&sql, &as_params(&params_v), TrackRow::from_row)?;
    Ok((total, rows))
}

pub fn query_ids(c: &Connection, filter: &Filter) -> Result<Vec<i64>, StoreError> {
    let (w, p) = filter.sql();
    Ok(c.query(&format!("SELECT id FROM tracks WHERE {w} ORDER BY sort_title, id"), &as_params(&p), |r| r.i64(0))?)
}

pub fn get_track(c: &Connection, id: i64) -> Result<Option<TrackRow>, StoreError> {
    Ok(c.query_row(&format!("SELECT {TRACK_COLUMNS} FROM tracks WHERE id = ?"), params![id], TrackRow::from_row)?)
}

pub fn get_tracks(c: &Connection, ids: &[i64]) -> Result<Vec<TrackRow>, StoreError> {
    let mut out = Vec::with_capacity(ids.len());
    for chunk in ids.chunks(500) {
        let marks = vec!["?"; chunk.len()].join(",");
        let p: Vec<SqlValue> = chunk.iter().map(|&i| SqlValue::Int(i)).collect();
        out.extend(c.query(&format!("SELECT {TRACK_COLUMNS} FROM tracks WHERE id IN ({marks})"), &as_params(&p), TrackRow::from_row)?);
    }
    Ok(out)
}

/// Full detail: the row plus the stored analysis documents.
/// A track with its stored analysis and history. `with_visuals: false`
/// leaves out the visualization data (most of the size) for quick previews.
pub fn track_detail(c: &Connection, id: i64, with_visuals: bool) -> Result<Option<Value>, StoreError> {
    let Some(row) = get_track(c, id)? else { return Ok(None) };
    let sql = if with_visuals {
        "SELECT facts, measurements, assessment, visuals, created_at FROM analyses WHERE track_id = ?"
    } else {
        "SELECT facts, measurements, assessment, NULL, created_at FROM analyses WHERE track_id = ?"
    };
    let analysis = c.query_row(sql, params![id], |r| (r.text(0), r.text(1), r.text(2), r.opt_text(3), r.i64(4)))?;
    let parse = |s: &str| sonora_json::parse(s).unwrap_or(Value::Null);
    let analysis_json = match analysis {
        Some((facts, meas, assess, vis, created)) => obj! {
            "facts" => parse(&facts),
            "measurements" => parse(&meas),
            "assessment" => parse(&assess),
            "visuals" => vis.map(|v| parse(&v)).unwrap_or(Value::Null),
            "createdAt" => created,
        },
        None => Value::Null,
    };
    let history = c.query(
        "SELECT s.id, s.kind, s.started_at, i.outcome, i.finished_at FROM scan_items i JOIN scans s ON s.id = i.scan_id WHERE i.track_id = ? ORDER BY s.started_at DESC LIMIT 20",
        params![id],
        |r| obj! { "scanId" => r.i64(0), "kind" => r.text(1), "startedAt" => r.i64(2), "outcome" => r.text(3), "finishedAt" => r.opt_i64(4) },
    )?;
    let exists = Path::new(&row.path).exists();
    Ok(Some(obj! { "track" => row, "analysis" => analysis_json, "history" => Value::Array(history), "fileExists" => exists }))
}

pub fn facets(c: &Connection) -> Result<Value, StoreError> {
    let formats = c.query("SELECT format, COUNT(*) FROM tracks WHERE format IS NOT NULL GROUP BY format ORDER BY COUNT(*) DESC", params![], |r| {
        obj! { "id" => r.text(0), "count" => r.i64(1) }
    })?;
    let statuses = c.query("SELECT status, COUNT(*) FROM tracks GROUP BY status", params![], |r| obj! { "id" => r.text(0), "count" => r.i64(1) })?;
    let missing = c.query_row("SELECT COUNT(*) FROM tracks WHERE missing_since IS NOT NULL", params![], |r| r.i64(0))?.unwrap_or(0);
    let mut flags = Vec::new();
    for f in FLAGS {
        let n = c.query_row("SELECT COUNT(*) FROM tracks WHERE flags LIKE ?", params![format!("%,{f},%")], |r| r.i64(0))?.unwrap_or(0);
        if n > 0 {
            flags.push(obj! { "id" => *f, "count" => n });
        }
    }
    let lossless = c.query_row("SELECT SUM(lossless = 1), SUM(lossless = 0) FROM tracks", params![], |r| (r.opt_i64(0).unwrap_or(0), r.opt_i64(1).unwrap_or(0)))?.unwrap_or((0, 0));
    let genres = c.query(
        "SELECT MIN(dj_genre), COUNT(*) FROM tracks WHERE dj_genre IS NOT NULL GROUP BY dj_genre COLLATE NOCASE ORDER BY COUNT(*) DESC, 1 LIMIT 300",
        params![],
        |r| obj! { "id" => r.text(0), "count" => r.i64(1) },
    )?;
    let keys = c.query("SELECT dj_key, COUNT(*) FROM tracks WHERE dj_key IS NOT NULL GROUP BY dj_key ORDER BY dj_key", params![], |r| obj! { "key" => r.i64(0), "count" => r.i64(1) })?;
    let dj_pending = c
        .query_row("SELECT COUNT(*) FROM tracks WHERE status = 'done' AND dj_version IS NULL AND missing_since IS NULL", params![], |r| r.i64(0))?
        .unwrap_or(0);
    Ok(obj! {
        "formats" => Value::Array(formats),
        "statuses" => Value::Array(statuses),
        "missing" => missing,
        "flags" => Value::Array(flags),
        "lossless" => lossless.0,
        "lossy" => lossless.1,
        "genres" => Value::Array(genres),
        "keys" => Value::Array(keys),
        "djPending" => dj_pending,
    })
}

/// Flags the interface can filter on.
pub const FLAGS: &[&str] = &[
    "attention",
    "review",
    "clipping",
    "lossy",
    "low_bitrate",
    "source_concern",
    "artifacts",
    "integrity",
    "mislabeled",
    "phase",
    "noise",
    "upsampled",
    "padded",
    "silent",
    "inconclusive",
    "bandwidth",
];

pub fn overview(c: &Connection) -> Result<Value, StoreError> {
    let (total, analyzed, pending, analyzing, failed) = c
        .query_row(
            "SELECT COUNT(*), SUM(status = 'done'), SUM(status = 'pending'), SUM(status = 'analyzing'), SUM(status = 'failed') FROM tracks",
            params![],
            |r| (r.i64(0), r.opt_i64(1).unwrap_or(0), r.opt_i64(2).unwrap_or(0), r.opt_i64(3).unwrap_or(0), r.opt_i64(4).unwrap_or(0)),
        )?
        .unwrap_or((0, 0, 0, 0, 0));
    let missing = c.query_row("SELECT COUNT(*) FROM tracks WHERE missing_since IS NOT NULL", params![], |r| r.i64(0))?.unwrap_or(0);
    let folders = c.query_row("SELECT COUNT(*) FROM folders", params![], |r| r.i64(0))?.unwrap_or(0);
    let (avg_o, avg_d, avg_l, total_secs) = c
        .query_row("SELECT AVG(overall), AVG(dj), AVG(listening), SUM(duration) FROM tracks WHERE status = 'done'", params![], |r| {
            (r.opt_f64(0), r.opt_f64(1), r.opt_f64(2), r.opt_f64(3))
        })?
        .unwrap_or((None, None, None, None));
    let count_flag = |f: &str| -> Result<i64, StoreError> {
        Ok(c.query_row("SELECT COUNT(*) FROM tracks WHERE status = 'done' AND flags LIKE ?", params![format!("%,{f},%")], |r| r.i64(0))?.unwrap_or(0))
    };
    let attention = c
        .query_row(
            "SELECT COUNT(*) FROM tracks WHERE flags LIKE '%,attention,%' OR flags LIKE '%,review,%' OR status = 'failed' OR missing_since IS NOT NULL",
            params![],
            |r| r.i64(0),
        )?
        .unwrap_or(0);
    let mut dist = Vec::new();
    for kind in ["overall", "dj", "listening"] {
        let mut bins = vec![0i64; 10];
        let rows = c.query(&format!("SELECT MIN(CAST({kind} / 10 AS INTEGER), 9), COUNT(*) FROM tracks WHERE status = 'done' AND {kind} IS NOT NULL GROUP BY 1"), params![], |r| {
            (r.i64(0), r.i64(1))
        })?;
        for (b, n) in rows {
            if (0..10).contains(&b) {
                bins[b as usize] = n;
            }
        }
        dist.push(obj! { "kind" => kind, "bins" => bins });
    }
    let formats = c.query("SELECT format, COUNT(*), AVG(overall) FROM tracks WHERE status = 'done' GROUP BY format ORDER BY COUNT(*) DESC", params![], |r| {
        obj! { "format" => r.text(0), "count" => r.i64(1), "avgOverall" => r.opt_f64(2).map(|v| num(v, 1)).unwrap_or(Value::Null) }
    })?;
    let recent_tracks = c.query(
        &format!("SELECT {TRACK_COLUMNS} FROM tracks WHERE status IN ('done','failed') AND analyzed_at IS NOT NULL ORDER BY analyzed_at DESC LIMIT 8"),
        params![],
        TrackRow::from_row,
    )?;
    let recent_scans = c.query(
        "SELECT id, kind, state, started_at, finished_at, total, completed, failed, skipped, roots FROM scans ORDER BY id DESC LIMIT 5",
        params![],
        scan_row_json,
    )?;
    Ok(obj! {
        "tracks" => obj! { "total" => total, "analyzed" => analyzed, "pending" => pending, "analyzing" => analyzing, "failed" => failed, "missing" => missing },
        "folders" => folders,
        "averages" => obj! {
            "overall" => avg_o.map(|v| num(v, 1)).unwrap_or(Value::Null),
            "dj" => avg_d.map(|v| num(v, 1)).unwrap_or(Value::Null),
            "listening" => avg_l.map(|v| num(v, 1)).unwrap_or(Value::Null),
        },
        "totalDuration" => total_secs.map(|v| num(v, 0)).unwrap_or(Value::Null),
        "attention" => attention,
        "clipping" => count_flag("clipping")?,
        "sourceConcerns" => count_flag("source_concern")?,
        "integrity" => count_flag("integrity")?,
        "lossy" => count_flag("lossy")?,
        "distribution" => Value::Array(dist),
        "formats" => Value::Array(formats),
        "recentTracks" => Value::Array(recent_tracks.iter().map(ToJson::to_json).collect()),
        "recentScans" => Value::Array(recent_scans),
    })
}

pub fn scan_row_json(r: &crate::db::sqlite::Row) -> Value {
    obj! {
        "id" => r.i64(0), "kind" => r.text(1), "state" => r.text(2), "startedAt" => r.i64(3), "finishedAt" => r.opt_i64(4),
        "total" => r.i64(5), "completed" => r.i64(6), "failed" => r.i64(7), "skipped" => r.i64(8),
        "roots" => sonora_json::parse(&r.text(9)).unwrap_or(Value::Array(Vec::new())),
    }
}

pub fn folders(c: &Connection) -> Result<Value, StoreError> {
    let rows = c.query(
        "SELECT f.id, f.path, f.added_at, f.last_scan_at, f.available, COUNT(t.id), SUM(t.status = 'done'), SUM(t.missing_since IS NOT NULL), \
         SUM(t.status = 'failed'), AVG(t.overall), SUM(t.size) \
         FROM folders f LEFT JOIN tracks t ON t.folder_id = f.id GROUP BY f.id ORDER BY f.path",
        params![],
        |r| {
            let path = r.text(1);
            obj! {
                "id" => r.i64(0), "path" => path.clone(), "name" => Path::new(&path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or(path.clone()),
                "addedAt" => r.i64(2), "lastScanAt" => r.opt_i64(3), "available" => r.bool(4) && Path::new(&path).is_dir(),
                "tracks" => r.i64(5), "analyzed" => r.opt_i64(6).unwrap_or(0), "missing" => r.opt_i64(7).unwrap_or(0),
                "failed" => r.opt_i64(8).unwrap_or(0), "avgOverall" => r.opt_f64(9).map(|v| num(v, 1)).unwrap_or(Value::Null),
                "bytes" => r.opt_i64(10).unwrap_or(0),
            }
        },
    )?;
    Ok(Value::Array(rows))
}

/// Removes index entries (never touches files on disk).
pub fn remove_tracks(c: &Connection, ids: &[i64]) -> Result<u64, StoreError> {
    let mut n = 0u64;
    c.transaction(|tx| {
        for chunk in ids.chunks(500) {
            let marks = vec!["?"; chunk.len()].join(",");
            let p: Vec<SqlValue> = chunk.iter().map(|&i| SqlValue::Int(i)).collect();
            n += tx.execute(&format!("DELETE FROM tracks WHERE id IN ({marks})"), &as_params(&p))? as u64;
        }
        Ok::<(), StoreError>(())
    })?;
    Ok(n)
}

pub fn remove_folder(c: &Connection, id: i64, remove_tracks_too: bool) -> Result<u64, StoreError> {
    c.transaction(|tx| {
        let mut n = 0;
        if remove_tracks_too {
            n = tx.execute("DELETE FROM tracks WHERE folder_id = ?", params![id])? as u64;
        }
        tx.execute("DELETE FROM folders WHERE id = ?", params![id])?;
        Ok::<u64, StoreError>(n)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_prefix_pattern_matches_only_paths_inside() {
        let c = Connection::open_in_memory().unwrap();
        // Both separators, whatever this platform uses: LIKE wildcards in a
        // folder name stay literal, and a backslash separator (Windows) must
        // not escape the trailing %.
        for (root, sep) in [("/music/100%_club", '/'), ("C:\\music\\100%_club", '\\')] {
            let pattern = format!("{}%", like_prefix(root, sep));
            let hit = |p: String| c.query_row("SELECT ? LIKE ? ESCAPE '\\'", params![p, pattern.as_str()], |r| r.i64(0)).unwrap() == Some(1);
            assert!(hit(format!("{root}{sep}a.wav")), "{pattern}");
            assert!(!hit(format!("{root}bing{sep}b.wav")), "{pattern}");
            assert!(!hit(root.replace("100%_club", "100x club") + &format!("{sep}c.wav")), "{pattern}");
        }
    }

    #[test]
    fn filter_sql_is_parameterised() {
        let f = Filter {
            search: Some("deep 100%".into()),
            score_kind: "dj".into(),
            min_score: Some(50.0),
            formats: vec!["FLAC".into(), "lossy".into()],
            flags: vec!["clipping".into()],
            statuses: vec!["failed".into(), "missing".into()],
            folder_id: Some(3),
            attention: true,
            ..Default::default()
        };
        let (w, p) = f.sql();
        assert!(w.contains("dj >= ?"));
        assert!(!w.contains("deep"), "user text never enters SQL");
        assert!(p.contains(&SqlValue::Text("%100\\%%".into())));
        assert!(w.contains("missing_since IS NOT NULL"));
        assert!(sort_expr("overall; DROP TABLE tracks").is_none());
    }
}
