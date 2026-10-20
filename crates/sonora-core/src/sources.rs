//! Music downloaded with the person's own Soulseek client.
//!
//! SONORA watches the folder where that client saves finished downloads and
//! adds what arrives to the library, noting where each file came from. It
//! never connects to the Soulseek network, holds no Soulseek password and
//! never shares, moves, renames or deletes a file there: the client does the
//! downloading and the sharing, and SONORA only reads finished files.
//!
//! A file counts as finished when its name and folder are not the ones
//! clients use for partial downloads, its size and modification time are
//! the same at two looks in a row, its last change is at least
//! [`SETTLE_MS`] old, and it can be opened for reading. Files the person
//! removed from the library stay out, as they do for rescans.

use crate::app::App;
use crate::db::sqlite::Connection;
use crate::db::{now_ms, Db, StoreError};
use crate::events::EventBus;
use crate::index::{self, WalkOptions};
use crate::jobs::IndexJob;
use crate::library::{self, ImportReport};
use crate::params;
use sonora_json::{obj, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak};
use std::time::Duration;

/// The only source so far: the person's own Soulseek client.
pub const SOULSEEK: &str = "soulseek";

/// How old a file's last change must be before SONORA takes it.
pub const SETTLE_MS: i64 = 10_000;

/// Between two looks at the folder, unless `SONORA_SOURCE_POLL_SECONDS`
/// says otherwise (0 = only when asked, for tests).
const POLL: Duration = Duration::from_secs(20);

/// An import that has not reported back after this long is tried again
/// (SONORA quit or crashed while it ran).
const STALE_IMPORT_MS: i64 = 10 * 60_000;

/// Most files handed to one import, so a folder of thousands does not hold
/// up everything else; the rest follow at the next look.
const BATCH: usize = 300;

/// Lawful-use notice accepted (the time, in `meta`).
const ACCEPTED_KEY: &str = "lawful_use_accepted";
/// The reminder shown with the first import from Soulseek was given.
const REMINDED_KEY: &str = "soulseek_first_import";

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

#[derive(Clone, Debug, Default)]
struct Status {
    last_check: Option<i64>,
    error: Option<String>,
    /// Files seen at the last look that are not finished yet.
    settling: usize,
}

#[derive(Default)]
struct Inner {
    stop: AtomicBool,
    woken: Mutex<bool>,
    wake: Condvar,
    /// Size and modification time of each file at the last look.
    looks: Mutex<HashMap<PathBuf, (u64, i64)>>,
    /// One look at a time (the watcher and "Check now").
    checking: Mutex<()>,
    status: Mutex<Status>,
}

/// The folder watcher. Idle until the lawful-use notice is accepted and a
/// folder is chosen.
#[derive(Default)]
pub struct Watcher {
    inner: Arc<Inner>,
}

impl Watcher {
    /// Starts looking at the folder in the background.
    pub fn start(app: &Arc<App>) {
        let poll = match std::env::var("SONORA_SOURCE_POLL_SECONDS").ok().and_then(|v| v.parse::<u64>().ok()) {
            Some(0) => return,
            Some(s) => Duration::from_secs(s),
            None => POLL,
        };
        let inner = Arc::clone(&app.sources.inner);
        let weak: Weak<App> = Arc::downgrade(app);
        let _ = std::thread::Builder::new().name("sonora-sources".into()).spawn(move || loop {
            {
                let mut woken = lock(&inner.woken);
                if !*woken {
                    woken = inner.wake.wait_timeout(woken, poll).map(|r| r.0).unwrap_or_else(|p| p.into_inner().0);
                }
                *woken = false;
            }
            if inner.stop.load(Ordering::Relaxed) {
                break;
            }
            let Some(app) = weak.upgrade() else { break };
            check(&app);
        });
    }

    /// Looks again soon (after the settings changed).
    pub fn wake(&self) {
        lock(&self.inner.looks).clear();
        *lock(&self.inner.woken) = true;
        self.inner.wake.notify_all();
    }

    pub fn stop(&self) {
        self.inner.stop.store(true, Ordering::Relaxed);
        *lock(&self.inner.woken) = true;
        self.inner.wake.notify_all();
    }
}

/// Whether the person accepted the lawful-use notice.
pub fn accepted_at(c: &Connection) -> Option<i64> {
    c.query_row("SELECT value FROM meta WHERE key = ?", params![ACCEPTED_KEY], |r| r.text(0)).ok().flatten().and_then(|v| v.parse().ok())
}

pub fn accept(c: &Connection) -> Result<i64, StoreError> {
    let now = now_ms();
    c.execute("INSERT OR REPLACE INTO meta (key, value) VALUES (?, ?)", params![ACCEPTED_KEY, now.to_string()])?;
    Ok(now)
}

/// Whether a path is one of the clients' partial downloads: inside a folder
/// for partial files directly in the watched folder ("incomplete",
/// "Incomplete Downloads", "temp", "tmp"), named the way Nicotine+ names
/// them ("INCOMPLETE" and a 32-digit hash before the name), or ending in a
/// partial-file extension (.part, .tmp…). Folders further down keep their
/// names: an album may well be called "Incomplete".
fn is_partial(path: &Path, root: &Path) -> bool {
    let inside = path.strip_prefix(root).unwrap_or(path);
    let parts: Vec<String> = inside.components().map(|c| c.as_os_str().to_string_lossy().to_lowercase()).collect();
    let Some((name, folders)) = parts.split_last() else { return false };
    if folders.first().is_some_and(|f| matches!(f.as_str(), "incomplete" | "incomplete downloads" | "temp" | "tmp")) {
        return true;
    }
    let nicotine = name.strip_prefix("incomplete").is_some_and(|rest| rest.len() > 32 && rest.as_bytes()[..32].iter().all(u8::is_ascii_hexdigit));
    nicotine || [".part", ".partial", ".tmp", ".incomplete", ".crdownload", ".download"].iter().any(|s| name.ends_with(s))
}

/// The file can be opened for reading (on Windows a file still being
/// written is often locked).
fn readable(path: &Path) -> bool {
    std::fs::File::open(path).is_ok()
}

/// What one look found.
#[derive(Clone, Debug, Default)]
pub struct CheckReport {
    /// Handed to the library now.
    pub imported: usize,
    /// Held for the person to add or keep out.
    pub held: usize,
    /// Not finished yet.
    pub settling: usize,
    pub error: Option<String>,
}

/// Looks at the folder once: notes new files, and adds (or holds) the
/// finished ones.
pub fn check(app: &App) -> CheckReport {
    let inner = &app.sources.inner;
    let _one = lock(&inner.checking);
    let report = match look(app) {
        Ok(r) => r,
        Err(e) => CheckReport { error: Some(e), ..CheckReport::default() },
    };
    let mut st = lock(&inner.status);
    st.last_check = Some(now_ms());
    st.error = report.error.clone();
    st.settling = report.settling;
    report
}

fn look(app: &App) -> Result<CheckReport, String> {
    let settings = app.settings();
    let mut out = CheckReport::default();
    let db_err = |e: StoreError| format!("the library could not be read: {e}");
    if accepted_at(&app.db.read()).is_none() {
        return Ok(out);
    }
    let Some(folder) = settings.soulseek_folder.clone() else { return Ok(out) };
    let root = PathBuf::from(&folder);
    if !root.is_dir() {
        return Err(format!("The folder {folder} is not available. Is its drive connected?"));
    }

    // Every audio file in the folder, partial downloads left out.
    let mut found = Vec::new();
    let never = AtomicBool::new(false);
    index::walk(&root, WalkOptions { skip_hidden: true, follow_symlinks: false }, &never, |c| {
        if !is_partial(&c.path, &root) {
            found.push(c);
        }
    });

    let pattern = format!("{}%", library::under_pattern(&root));
    let (known, removed) = {
        let c = app.db.read();
        let known: HashMap<String, (i64, i64, String, i64)> = c
            .query("SELECT path, size, mtime, state, found_at FROM source_files WHERE path LIKE ? ESCAPE '\\'", params![pattern], |r| {
                (r.text(0), (r.i64(1), r.i64(2), r.text(3), r.i64(4)))
            })
            .map_err(|e| db_err(e.into()))?
            .into_iter()
            .collect();
        let removed: HashSet<String> =
            c.query("SELECT path FROM removed_files WHERE path LIKE ? ESCAPE '\\'", params![pattern], |r| r.text(0)).map_err(|e| db_err(e.into()))?.into_iter().collect();
        (known, removed)
    };

    let now = now_ms();
    let mut ready = Vec::new();
    let mut already = Vec::new();
    {
        let mut looks = lock(&app.sources.inner.looks);
        let present: HashSet<PathBuf> = found.iter().map(|c| c.path.clone()).collect();
        looks.retain(|p, _| present.contains(p));
        for cand in found {
            let p = library::path_str(&cand.path);
            // Removed from the library by the person: it stays out.
            if removed.contains(&p) {
                continue;
            }
            if let Some((size, mtime, state, found_at)) = known.get(&p) {
                let same = *size == cand.size as i64 && *mtime == cand.mtime;
                // Handled already, or an import still on its way.
                if same && (state != "importing" || now - found_at < STALE_IMPORT_MS) {
                    continue;
                }
            }
            // Already in the library (the folder is inside a library folder, say).
            let track = app.db.read().query_row("SELECT id FROM tracks WHERE path = ?", params![p.clone()], |r| r.i64(0)).map_err(|e| db_err(e.into()))?;
            if let Some(id) = track {
                already.push((cand, id));
                continue;
            }
            let before = looks.insert(cand.path.clone(), (cand.size, cand.mtime));
            let settled = before == Some((cand.size, cand.mtime)) && now - cand.mtime >= SETTLE_MS && readable(&cand.path);
            if settled {
                ready.push(cand);
            } else {
                out.settling += 1;
            }
        }
    }

    let held = settings.soulseek_review;
    if !already.is_empty() || !ready.is_empty() {
        let c = app.db.write();
        let upsert = "INSERT INTO source_files (path, source, size, mtime, state, track_id, found_at, decided_at, error) VALUES (?, ?, ?, ?, ?, ?, ?, ?, NULL) \
                      ON CONFLICT(path) DO UPDATE SET source = excluded.source, size = excluded.size, mtime = excluded.mtime, state = excluded.state, \
                      track_id = excluded.track_id, found_at = excluded.found_at, decided_at = excluded.decided_at, error = NULL";
        c.transaction(|tx| {
            for (cand, id) in &already {
                tx.execute(upsert, params![library::path_str(&cand.path), SOULSEEK, cand.size as i64, cand.mtime, "imported", *id, now, now])?;
            }
            for cand in ready.iter().take(BATCH) {
                let state = if held { "review" } else { "importing" };
                tx.execute(upsert, params![library::path_str(&cand.path), SOULSEEK, cand.size as i64, cand.mtime, state, Option::<i64>::None, now, Option::<i64>::None])?;
            }
            Ok::<(), StoreError>(())
        })
        .map_err(db_err)?;
    }
    let ready: Vec<PathBuf> = ready.into_iter().take(BATCH).map(|c| c.path).collect();
    if held {
        out.held = ready.len();
    } else if !ready.is_empty() {
        out.imported = ready.len();
        app.scans.start_indexing(SOULSEEK, IndexJob::Source { files: ready.clone(), source: SOULSEEK.to_string() });
    }
    if !ready.is_empty() || !already.is_empty() {
        app.events.emit("sources.changed", &counts(&app.db.read()).unwrap_or(Value::Null));
    }
    Ok(out)
}

/// After an import of downloaded files: what became of each one.
pub fn record_imports(db: &Db, events: &EventBus, source: &str, files: &[PathBuf], report: &ImportReport) {
    let mut added = 0i64;
    let mut failed = 0i64;
    let mut first = false;
    {
        let c = db.write();
        let now = now_ms();
        for f in files {
            let p = library::path_str(f);
            let track = c.query_row("SELECT id FROM tracks WHERE path = ?", params![p.clone()], |r| r.i64(0)).ok().flatten();
            let result = match track {
                Some(id) => {
                    added += 1;
                    c.execute("UPDATE source_files SET state = 'imported', track_id = ?, decided_at = ?, error = NULL WHERE path = ?", params![id, now, p])
                }
                None => {
                    failed += 1;
                    let why = report.errors.iter().find(|e| e.starts_with(&p)).cloned().unwrap_or_else(|| "SONORA could not read it as audio".to_string());
                    c.execute("UPDATE source_files SET state = 'failed', decided_at = ?, error = ? WHERE path = ?", params![now, why, p])
                }
            };
            if let Err(e) = result {
                crate::logging::log("error", "sources", &format!("could not record an import: {e}"));
            }
        }
        if added > 0 && c.query_row("SELECT value FROM meta WHERE key = ?", params![REMINDED_KEY], |r| r.text(0)).ok().flatten().is_none() {
            first = c.execute("INSERT OR REPLACE INTO meta (key, value) VALUES (?, ?)", params![REMINDED_KEY, now.to_string()]).is_ok();
        }
    }
    crate::logging::log_fields("info", "sources", "imported downloads", obj! { "source" => source, "added" => added, "failed" => failed });
    events.emit("sources.imported", &obj! { "source" => source, "added" => added, "failed" => failed, "first" => first });
    events.emit("sources.changed", &counts(&db.read()).unwrap_or(Value::Null));
}

fn counts(c: &Connection) -> Result<Value, StoreError> {
    let mut v = obj! { "review" => 0, "imported" => 0, "failed" => 0, "declined" => 0, "importing" => 0 };
    for (state, n) in c.query("SELECT state, COUNT(*) FROM source_files WHERE state != 'existing' GROUP BY state", params![], |r| (r.text(0), r.i64(1)))? {
        v.set(&state, n);
    }
    Ok(v)
}

/// Files that were in the folder when it was chosen and that the library
/// does not have (nor had them removed): they stay out unless added.
fn existing_paths(c: &Connection, folder: &Path) -> Result<Vec<String>, StoreError> {
    let pattern = format!("{}%", library::under_pattern(folder));
    Ok(c.query(
        "SELECT s.path FROM source_files s WHERE s.state = 'existing' AND s.path LIKE ? ESCAPE '\\' \
         AND NOT EXISTS (SELECT 1 FROM tracks t WHERE t.path = s.path) AND NOT EXISTS (SELECT 1 FROM removed_files r WHERE r.path = s.path) ORDER BY s.path",
        params![pattern],
        |r| r.text(0),
    )?)
}

/// Notes the audio files already in a folder when it is chosen. They were
/// not downloaded since, so they are neither added nor marked as coming
/// from Soulseek unless the person adds them.
pub fn note_existing(app: &App, folder: &Path) -> Result<(), StoreError> {
    let mut found = Vec::new();
    let never = AtomicBool::new(false);
    index::walk(folder, WalkOptions { skip_hidden: true, follow_symlinks: false }, &never, |c| {
        if !is_partial(&c.path, folder) {
            found.push(c);
        }
    });
    let now = now_ms();
    app.db.write().transaction(|tx| {
        for cand in &found {
            tx.execute(
                "INSERT OR IGNORE INTO source_files (path, source, size, mtime, state, track_id, found_at, decided_at, error) VALUES (?, ?, ?, ?, 'existing', NULL, ?, NULL, NULL)",
                params![library::path_str(&cand.path), SOULSEEK, cand.size as i64, cand.mtime, now],
            )?;
        }
        Ok::<(), StoreError>(())
    })
}

/// Adds the files that were in the folder before it was chosen (or holds
/// them for review). Returns how many.
pub fn add_existing(app: &App) -> Result<usize, StoreError> {
    let settings = app.settings();
    let Some(folder) = settings.soulseek_folder.clone() else { return Ok(0) };
    let paths = existing_paths(&app.db.read(), Path::new(&folder))?;
    if paths.is_empty() {
        return Ok(0);
    }
    let now = now_ms();
    let state = if settings.soulseek_review { "review" } else { "importing" };
    app.db.write().transaction(|tx| {
        for p in &paths {
            tx.execute("UPDATE source_files SET state = ?, found_at = ? WHERE path = ?", params![state, now, p.clone()])?;
        }
        Ok::<(), StoreError>(())
    })?;
    if !settings.soulseek_review {
        app.scans.start_indexing(SOULSEEK, IndexJob::Source { files: paths.iter().map(PathBuf::from).collect(), source: SOULSEEK.to_string() });
    }
    app.events.emit("sources.changed", &counts(&app.db.read())?);
    Ok(paths.len())
}

/// What the interface shows about the watcher.
pub fn status_json(app: &App) -> Result<Value, StoreError> {
    let settings = app.settings();
    let c = app.db.read();
    let accepted = accepted_at(&c);
    let st = lock(&app.sources.inner.status).clone();
    let available = settings.soulseek_folder.as_deref().is_some_and(|f| Path::new(f).is_dir());
    Ok(obj! {
        "source" => SOULSEEK,
        "accepted" => accepted.is_some(),
        "acceptedAt" => accepted,
        "folder" => settings.soulseek_folder.clone().map(Value::String).unwrap_or(Value::Null),
        "review" => settings.soulseek_review,
        "watching" => accepted.is_some() && available,
        "lastCheck" => st.last_check,
        "error" => st.error.map(Value::String).unwrap_or(Value::Null),
        "settling" => st.settling as i64,
        "existing" => match settings.soulseek_folder.as_deref() {
            Some(f) => existing_paths(&c, Path::new(f))?.len(),
            None => 0,
        },
        "counts" => counts(&c)?,
    })
}

/// Files waiting for the person to add or keep out, newest first.
pub fn review_list(c: &Connection, limit: i64) -> Result<Value, StoreError> {
    let rows = c.query("SELECT path, size, found_at FROM source_files WHERE state = 'review' ORDER BY found_at DESC, path LIMIT ?", params![limit], |r| {
        let path = r.text(0);
        let name = Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        obj! { "path" => path, "name" => name, "size" => r.i64(1), "foundAt" => r.i64(2) }
    })?;
    Ok(Value::Array(rows))
}

/// Adds the files held for review, or keeps them out. Returns how many
/// went each way (files no longer held are passed over).
pub fn decide(app: &App, paths: &[String], add: bool) -> Result<(usize, usize), StoreError> {
    let now = now_ms();
    let mut chosen = Vec::new();
    {
        let c = app.db.write();
        for p in paths {
            let held = c.query_row("SELECT 1 FROM source_files WHERE path = ? AND state = 'review'", params![p.clone()], |r| r.i64(0))?.is_some();
            if !held {
                continue;
            }
            if add {
                c.execute("UPDATE source_files SET state = 'importing', found_at = ? WHERE path = ?", params![now, p.clone()])?;
            } else {
                c.execute("UPDATE source_files SET state = 'declined', decided_at = ? WHERE path = ?", params![now, p.clone()])?;
            }
            chosen.push(PathBuf::from(p));
        }
    }
    let n = chosen.len();
    if add && n > 0 {
        app.scans.start_indexing(SOULSEEK, IndexJob::Source { files: chosen, source: SOULSEEK.to_string() });
    }
    app.events.emit("sources.changed", &counts(&app.db.read())?);
    Ok(if add { (n, 0) } else { (0, n) })
}

/// Where a track came from, for its details: nothing for tracks added from
/// library folders or by hand.
pub fn origin_json(c: &Connection, track_id: i64) -> Result<Value, StoreError> {
    let row = c.query_row("SELECT source, path, found_at, decided_at FROM source_files WHERE track_id = ? ORDER BY found_at DESC LIMIT 1", params![track_id], |r| {
        (r.text(0), r.text(1), r.i64(2), r.opt_i64(3))
    })?;
    Ok(match row {
        Some((source, path, found, added)) => obj! {
            "source" => source,
            "originalName" => Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            "foundAt" => found,
            "addedAt" => added,
        },
        None => Value::Null,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_downloads_are_recognised() {
        let root = Path::new("/music/downloads");
        let nicotine = "user/INCOMPLETE0123456789abcdef0123456789abcdef02 Song.flac";
        for p in ["incomplete/a.flac", "Incomplete/user/b.mp3", "Incomplete Downloads/x.flac", nicotine, "c.flac.part", "d.mp3.tmp", "e.flac.incomplete", "temp/g.wav", "tmp/h.aiff"] {
            assert!(is_partial(&root.join(p), root), "{p}");
        }
        // Albums and songs may have these words in their names.
        for p in ["a.flac", "user/album/02 Song.mp3", "complete/x.wav", "parts/a.flac", "temperance/b.flac", "Artist/Incomplete/01 Song.flac", "Incomplete Love.flac", "Artist/tmp/x.flac", "~intro.flac"] {
            assert!(!is_partial(&root.join(p), root), "{p}");
        }
    }
}
