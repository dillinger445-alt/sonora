//! Background scanning: indexing and a bounded pool of analysis workers.
//!
//! - One scan session is active at a time; new imports while a scan runs are
//!   appended to it, so the Import Queue is a single, predictable list.
//! - Workers *claim* one queued item at a time from the database, so the
//!   queue is bounded by design and the collection is never loaded into
//!   memory.
//! - Pause takes effect between files (files already being analysed finish).
//! - Cancel stops running analyses promptly; every per-file write is a
//!   transaction, so cancellation never leaves partial results.
//! - A panic while analysing one file is caught and recorded as an internal
//!   error for that file; the worker carries on.
//! - On startup, sessions interrupted by a crash or forced quit are closed and
//!   their unfinished tracks returned to "pending".

use crate::db::sqlite::SqlValue;
use crate::db::{now_ms, Db, StoreError};
use crate::events::EventBus;
use crate::library::{self, ImportOptions, ImportReport};
use crate::model::{status, TrackRow};
use crate::params;
use crate::settings::Settings;
use sonora_engine::analysis::Control;
use sonora_engine::decode::ToolPaths;
use sonora_engine::{AnalysisError, AnalyzeOptions};
use sonora_json::{num, obj, ToJson, Value};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const PROGRESS_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Indexing,
    Running,
    Paused,
    Cancelling,
}

impl Phase {
    fn id(self) -> &'static str {
        match self {
            Phase::Idle => "idle",
            Phase::Indexing => "indexing",
            Phase::Running => "running",
            Phase::Paused => "paused",
            Phase::Cancelling => "cancelling",
        }
    }
}

/// What removing tracks from the library did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Removed {
    pub removed: u64,
    /// Tracks left in place because they are being analysed right now.
    pub busy: usize,
}

/// What moving tracks to the Trash did.
#[derive(Clone, Debug, Default)]
pub struct Trashed {
    /// Files moved to the Trash.
    pub trashed: u64,
    /// Tracks removed from the library: those whose files went, and those
    /// whose files were missing already.
    pub removed: u64,
    /// Tracks whose files stayed: (id, title, reason).
    pub failed: Vec<(i64, String, String)>,
    /// Tracks left alone because they are being analysed right now.
    pub busy: usize,
}

/// The ids not being analysed right now, and how many are.
fn split_busy(st: &RunState, ids: &[i64]) -> (Vec<i64>, usize) {
    let analysing: std::collections::HashSet<i64> = st.slots.iter().flatten().map(|s| s.track_id).collect();
    let free: Vec<i64> = ids.iter().copied().filter(|id| !analysing.contains(id)).collect();
    let busy = ids.len() - free.len();
    (free, busy)
}

#[derive(Clone, Debug)]
struct Slot {
    track_id: i64,
    file_name: String,
    path: String,
    started: Instant,
    fraction: f64,
}

struct RunState {
    scan_id: Option<i64>,
    phase: Phase,
    kind: String,
    started_at: i64,
    started: Instant,
    total: i64,
    completed: i64,
    failed: i64,
    skipped: i64,
    cancelled: i64,
    /// Queue items that ended as skipped (their track vanished meanwhile).
    /// Part of `skipped`, and counted as done for progress.
    dropped: i64,
    next_seq: i64,
    slots: Vec<Option<Slot>>,
    /// Which worker slots have a thread (a worker that found the queue
    /// empty ends; more work starts it again).
    alive: Vec<bool>,
    recent: VecDeque<f64>,
    workers_alive: usize,
    handles: Vec<JoinHandle<()>>,
    indexing: Option<String>,
    pending_index_jobs: usize,
    last_emit: Instant,
    paused_at: Option<Instant>,
    paused_total: Duration,
}

impl RunState {
    fn new() -> Self {
        RunState {
            scan_id: None,
            phase: Phase::Idle,
            kind: String::new(),
            started_at: 0,
            started: Instant::now(),
            total: 0,
            completed: 0,
            failed: 0,
            skipped: 0,
            cancelled: 0,
            dropped: 0,
            next_seq: 0,
            slots: Vec::new(),
            alive: Vec::new(),
            recent: VecDeque::new(),
            workers_alive: 0,
            handles: Vec::new(),
            indexing: None,
            pending_index_jobs: 0,
            last_emit: Instant::now() - PROGRESS_INTERVAL,
            paused_at: None,
            paused_total: Duration::ZERO,
        }
    }
}

impl RunState {
    /// Adds `delta` to the live counter an item outcome is counted in.
    fn count(&mut self, outcome: &str, delta: i64) {
        match counter_of(outcome) {
            "completed" => self.completed += delta,
            "failed" => self.failed += delta,
            "cancelled" => self.cancelled += delta,
            _ => {
                self.skipped += delta;
                self.dropped += delta;
            }
        }
    }
}

/// The `scans` column (and live counter) an item outcome is counted in.
fn counter_of(outcome: &str) -> &'static str {
    match outcome {
        "done" => "completed",
        "failed" => "failed",
        "cancelled" | "interrupted" => "cancelled",
        _ => "skipped",
    }
}

/// What queueing tracks did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Queued {
    /// Tracks put in the queue, including ones this session had already
    /// finished and now analyses again.
    pub queued: i64,
    /// Tracks that were already waiting or being analysed in this session.
    pub already: i64,
    /// Nothing was queued because a cancelled session is still stopping.
    pub refused: bool,
}

pub struct Shared {
    pub db: Arc<Db>,
    pub events: Arc<EventBus>,
    pub tools: RwLock<ToolPaths>,
    pub settings: RwLock<Settings>,
    state: Mutex<RunState>,
    wake: Condvar,
    cancel: AtomicBool,
    index_cancel: AtomicBool,
    /// Imports are numbered as they are asked for; Cancel stops every one
    /// asked for before it, also those still waiting for their turn (B30).
    index_seq: std::sync::atomic::AtomicU64,
    index_cancel_upto: std::sync::atomic::AtomicU64,
    shutdown: AtomicBool,
    index_lock: Mutex<()>,
}

pub struct ScanManager {
    pub shared: Arc<Shared>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl ScanManager {
    pub fn new(db: Arc<Db>, events: Arc<EventBus>, tools: ToolPaths, settings: Settings) -> ScanManager {
        ScanManager {
            shared: Arc::new(Shared {
                db,
                events,
                tools: RwLock::new(tools),
                settings: RwLock::new(settings),
                state: Mutex::new(RunState::new()),
                wake: Condvar::new(),
                cancel: AtomicBool::new(false),
                index_cancel: AtomicBool::new(false),
                index_seq: std::sync::atomic::AtomicU64::new(0),
                index_cancel_upto: std::sync::atomic::AtomicU64::new(0),
                shutdown: AtomicBool::new(false),
                index_lock: Mutex::new(()),
            }),
        }
    }

    /// Closes sessions left open by a crash and returns their tracks to pending.
    pub fn recover(db: &Db) -> Result<u64, StoreError> {
        let c = db.write();
        c.transaction(|tx| {
            tx.execute("UPDATE scans SET state = 'interrupted', finished_at = ? WHERE state IN ('running', 'paused', 'indexing', 'cancelling')", params![now_ms()])?;
            tx.execute("UPDATE scan_items SET outcome = 'interrupted' WHERE outcome IN ('queued', 'running')", params![])?;
            let n = tx.execute(
                "UPDATE tracks SET status = CASE WHEN id IN (SELECT track_id FROM analyses) THEN 'done' ELSE 'pending' END WHERE status = 'analyzing'",
                params![],
            )?;
            Ok::<u64, StoreError>(n as u64)
        })
    }

    pub fn import_options(&self) -> ImportOptions {
        let s = self.shared.settings.read().unwrap_or_else(|p| p.into_inner());
        ImportOptions {
            walk: crate::index::WalkOptions { skip_hidden: s.skip_hidden, follow_symlinks: s.follow_symlinks },
            reanalyze_changed: s.reanalyze_changed,
            force: false,
        }
    }

    /// Indexes paths in the background, then queues what needs analysis.
    /// `job` decides between importing paths and rescanning folders.
    pub fn start_indexing(&self, kind: &str, job: IndexJob) {
        let shared = Arc::clone(&self.shared);
        let kind = kind.to_string();
        let opts = self.import_options();
        let seq = shared.index_seq.fetch_add(1, Ordering::SeqCst) + 1;
        {
            let mut st = lock(&shared.state);
            st.pending_index_jobs += 1;
            if st.phase == Phase::Idle {
                st.phase = Phase::Indexing;
                st.kind = kind.clone();
            }
            st.indexing = Some(job.describe());
            emit_progress(&shared, &mut st, true);
        }
        std::thread::Builder::new()
            .name("sonora-index".into())
            .spawn(move || {
                let _serial = lock(&shared.index_lock);
                // Cancelled while it waited for the import before it.
                shared.index_cancel.store(seq <= shared.index_cancel_upto.load(Ordering::SeqCst), Ordering::SeqCst);
                let result = match &job {
                    IndexJob::Paths(p) => library::import_paths(&shared.db, p, &opts, &shared.index_cancel),
                    IndexJob::Folders(ids) => library::rescan_folders(&shared.db, ids.as_deref(), &opts, &shared.index_cancel),
                    IndexJob::Force(p) => {
                        let mut o = opts;
                        o.force = true;
                        library::import_paths(&shared.db, p, &o, &shared.index_cancel)
                    }
                    IndexJob::Source { files, .. } => library::import_paths(&shared.db, files, &opts, &shared.index_cancel),
                };
                let cancelled = shared.index_cancel.load(Ordering::Relaxed);
                // Queue first, then leave the indexing phase, so observers go
                // straight from "indexing" to "running" without an idle gap.
                match &result {
                    Ok(report) => {
                        // Queued before the report goes out, so the report can say
                        // when the files only wait (a cancelled scan was still stopping).
                        let refused = !cancelled && {
                            let q = enqueue(&shared, &kind, &report.queue, &report.roots, (report.unchanged + report.relinked) as i64);
                            if q.refused {
                                crate::logging::log("warn", "jobs", "indexed files were not queued because a cancelled scan was still stopping; they wait for analysis");
                            }
                            q.refused
                        };
                        let mut v = report_json(report, cancelled);
                        v.set("waiting", refused);
                        if let IndexJob::Source { files, source } = &job {
                            crate::sources::record_imports(&shared.db, &shared.events, source, files, report);
                            v.set("source", source.as_str());
                        }
                        shared.events.emit("library.indexed", &v);
                        shared.events.emit("library.changed", &obj! { "reason" => "indexed" });
                    }
                    Err(e) => {
                        log_error(&format!("indexing failed: {e}"));
                        shared.events.emit("library.indexed", &obj! { "error" => e.to_string() });
                    }
                }
                {
                    let mut st = lock(&shared.state);
                    st.pending_index_jobs = st.pending_index_jobs.saturating_sub(1);
                    if st.pending_index_jobs == 0 {
                        st.indexing = None;
                        if st.phase == Phase::Indexing {
                            st.phase = Phase::Idle;
                        }
                    }
                }
                let mut st = lock(&shared.state);
                emit_progress(&shared, &mut st, true);
            })
            .expect("spawn indexer");
    }

    /// Queues specific tracks for (re-)analysis.
    pub fn enqueue_tracks(&self, kind: &str, ids: &[i64]) -> Queued {
        enqueue(&self.shared, kind, ids, &[], 0)
    }

    /// Removes a library folder from the library, with its tracks when
    /// `with_tracks` (taken out of a running scan's queue too). Removing the
    /// tracks is refused (`None`) while folders are being indexed or one of
    /// its tracks is being analysed right now, checked under the state lock
    /// so neither can start meanwhile. Files on disk are never touched.
    pub fn remove_folder(&self, id: i64, with_tracks: bool) -> Result<Option<u64>, StoreError> {
        let mut st = lock(&self.shared.state);
        let mut n = 0;
        // Not while folders are being read: the reading adds tracks to the
        // folder it is removing (B72).
        if st.phase == Phase::Indexing || st.pending_index_jobs > 0 {
            return Ok(None);
        }
        if with_tracks {
            let ids = self.shared.db.read().query("SELECT id FROM tracks WHERE folder_id = ?", params![id], |r| r.i64(0))?;
            let (free, busy) = split_busy(&st, &ids);
            if busy > 0 {
                return Ok(None);
            }
            // Remembered as removed; the folder's own exclusions then go with
            // it, except files another library folder still covers, whose
            // rescans must leave them out too.
            n = self.remove_locked(&mut st, &free, true)?;
        }
        library::remove_folder(&self.shared.db.write(), id, false)?;
        drop(st);
        Ok(Some(n))
    }

    /// Removes tracks from the library (index entries only; files on disk
    /// are never touched), and rescans of their folders leave the files out
    /// from then on. Queued items of removed tracks leave the running
    /// session's total, so its progress still ends at 100 %. Tracks being
    /// analysed right now stay; the result counts them as `busy`.
    pub fn remove_tracks(&self, ids: &[i64]) -> Result<Removed, StoreError> {
        // Holding the state lock keeps workers from claiming items meanwhile.
        let mut st = lock(&self.shared.state);
        let (free, busy) = split_busy(&st, ids);
        let removed = self.remove_locked(&mut st, &free, true)?;
        Ok(Removed { removed, busy })
    }

    /// Moves the tracks' files to the system's Trash, then removes from the
    /// library the tracks whose files went there (or were missing already).
    /// A file that cannot go to the Trash stays where it is, and its track
    /// stays in the library; so do tracks being analysed right now (`busy`).
    pub fn trash_tracks(&self, ids: &[i64]) -> Result<Trashed, StoreError> {
        // Under the state lock: leave out tracks being analysed, and take the
        // others out of the scan's queue so no worker starts on a file that
        // is about to go.
        let (rows, busy, dequeued, kind) = {
            let mut st = lock(&self.shared.state);
            let (free, busy) = split_busy(&st, ids);
            let dequeued = self.dequeue_locked(&mut st, &free)?;
            (library::get_tracks(&self.shared.db.read(), &free)?, busy, dequeued, st.kind.clone())
        };
        // Moving can take a while (a USB stick, a network share): without the
        // lock, so analysis and status calls carry on meanwhile.
        let mut out = Trashed { busy, ..Trashed::default() };
        let mut gone = Vec::new();
        for r in &rows {
            let p = Path::new(&r.path);
            match std::fs::symlink_metadata(p) {
                Ok(_) => {}
                // Deleted elsewhere already: the track just leaves the library.
                // Its folder must be there, though: a disconnected drive hides
                // files that still exist.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound && (r.missing_since.is_some() || p.parent().is_some_and(Path::is_dir)) => {
                    gone.push(r.id);
                    continue;
                }
                Err(_) => {
                    let folder = p.parent().map(|f| f.display().to_string()).unwrap_or_default();
                    out.failed.push((r.id, r.display_title(), format!("its folder {folder} is not available (is the drive connected?)")));
                    continue;
                }
            }
            match crate::trash::move_to_trash(p) {
                Ok(()) => {
                    out.trashed += 1;
                    gone.push(r.id);
                }
                Err(e) => out.failed.push((r.id, r.display_title(), e)),
            }
        }
        // Tracks whose files stayed go back into the queue they were taken out of.
        let stayed: Vec<i64> = dequeued.iter().copied().filter(|id| out.failed.iter().any(|f| f.0 == *id)).collect();
        if !stayed.is_empty() {
            enqueue(&self.shared, &kind, &stayed, &[], 0);
        }
        let mut st = lock(&self.shared.state);
        // A track queued again and started meanwhile keeps its row until its
        // analysis ends (it will find the file gone); say so.
        let (free, late) = split_busy(&st, &gone);
        if late > 0 {
            for r in rows.iter().filter(|r| gone.contains(&r.id) && !free.contains(&r.id)) {
                out.failed.push((r.id, r.display_title(), "its file is in the Trash, but the track was being analyzed: remove it from the library once that ends".into()));
            }
        }
        // The files are in the Trash: one put back is welcome again.
        out.removed = self.remove_locked(&mut st, &free, false)?;
        Ok(out)
    }

    /// Takes tracks out of the running scan's queue (not ones being
    /// analysed); returns the ones that were waiting in it.
    fn dequeue_locked(&self, st: &mut RunState, ids: &[i64]) -> Result<Vec<i64>, StoreError> {
        let Some(scan_id) = st.scan_id else { return Ok(Vec::new()) };
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let c = self.shared.db.write();
        let mut dequeued = Vec::new();
        for chunk in ids.chunks(500) {
            let marks = vec!["?"; chunk.len()].join(",");
            let mut p = vec![SqlValue::Int(scan_id)];
            p.extend(chunk.iter().map(|&i| SqlValue::Int(i)));
            let waiting = c.query(&format!("SELECT track_id FROM scan_items WHERE scan_id = ? AND outcome = 'queued' AND track_id IN ({marks})"), &library::as_params(&p), |r| r.i64(0))?;
            c.execute(&format!("DELETE FROM scan_items WHERE scan_id = ? AND outcome = 'queued' AND track_id IN ({marks})"), &library::as_params(&p))?;
            dequeued.extend(waiting);
        }
        let n = dequeued.len() as i64;
        if n > 0 {
            c.execute("UPDATE scans SET total = MAX(0, total - ?) WHERE id = ?", params![n, scan_id])?;
            st.total = (st.total - n).max(0);
        }
        drop(c);
        if n > 0 {
            emit_progress(&self.shared, st, true);
        }
        Ok(dequeued)
    }

    fn remove_locked(&self, st: &mut RunState, ids: &[i64], remember: bool) -> Result<u64, StoreError> {
        if ids.is_empty() {
            return Ok(0);
        }
        self.dequeue_locked(st, ids)?;
        library::remove_tracks(&self.shared.db.write(), ids, remember)
    }

    pub fn pause(&self) -> bool {
        let mut st = lock(&self.shared.state);
        if st.phase != Phase::Running {
            return false;
        }
        st.phase = Phase::Paused;
        st.paused_at = Some(Instant::now());
        set_scan_state(&self.shared.db, st.scan_id, "paused");
        emit_progress(&self.shared, &mut st, true);
        true
    }

    pub fn resume(&self) -> bool {
        let mut st = lock(&self.shared.state);
        if st.phase != Phase::Paused {
            return false;
        }
        st.phase = Phase::Running;
        if let Some(p) = st.paused_at.take() {
            st.paused_total += p.elapsed();
        }
        set_scan_state(&self.shared.db, st.scan_id, "running");
        self.shared.wake.notify_all();
        emit_progress(&self.shared, &mut st, true);
        true
    }

    pub fn cancel(&self) -> bool {
        let mut st = lock(&self.shared.state);
        self.shared.index_cancel_upto.store(self.shared.index_seq.load(Ordering::SeqCst), Ordering::SeqCst);
        self.shared.index_cancel.store(true, Ordering::SeqCst);
        if st.scan_id.is_none() {
            return st.phase == Phase::Indexing;
        }
        self.shared.cancel.store(true, Ordering::Relaxed);
        st.phase = Phase::Cancelling;
        self.shared.wake.notify_all();
        if st.workers_alive == 0 {
            // Normally the last worker closes the session; without workers, close it here.
            finish_scan(&self.shared, &mut st);
            return true;
        }
        emit_progress(&self.shared, &mut st, true);
        true
    }

    pub fn status(&self) -> Value {
        let st = lock(&self.shared.state);
        status_json(&self.shared, &st)
    }

    /// Stops everything and waits (up to `timeout`) for workers to exit.
    pub fn shutdown(&self, timeout: Duration) {
        self.shared.shutdown.store(true, Ordering::Relaxed);
        self.cancel();
        let handles: Vec<JoinHandle<()>> = std::mem::take(&mut lock(&self.shared.state).handles);
        let deadline = Instant::now() + timeout;
        for h in handles {
            while !h.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            if h.is_finished() {
                let _ = h.join();
            }
        }
    }

    pub fn is_busy(&self) -> bool {
        lock(&self.shared.state).phase != Phase::Idle
    }
}

#[derive(Clone, Debug)]
pub enum IndexJob {
    Paths(Vec<PathBuf>),
    Folders(Option<Vec<i64>>),
    /// Re-analyse everything under these paths.
    Force(Vec<PathBuf>),
    /// Files a download client finished (the person's own Soulseek
    /// client): imported one by one, with where they came from.
    Source { files: Vec<PathBuf>, source: String },
}

impl IndexJob {
    fn describe(&self) -> String {
        match self {
            IndexJob::Paths(p) | IndexJob::Force(p) if p.len() == 1 => format!("Indexing {}", p[0].display()),
            IndexJob::Paths(p) | IndexJob::Force(p) => format!("Indexing {} locations", p.len()),
            IndexJob::Folders(_) => "Checking library folders for changes".to_string(),
            IndexJob::Source { files, .. } if files.len() == 1 => "Adding a new download".to_string(),
            IndexJob::Source { files, .. } => format!("Adding {} new downloads", files.len()),
        }
    }
}

fn report_json(r: &ImportReport, cancelled: bool) -> Value {
    let mut v = r.to_json();
    v.set("cancelled", cancelled);
    v
}

fn log_error(msg: &str) {
    crate::logging::log("error", "jobs", msg);
}

fn set_scan_state(db: &Db, scan_id: Option<i64>, state: &str) {
    if let Some(id) = scan_id {
        let _ = db.write().execute("UPDATE scans SET state = ? WHERE id = ?", params![state, id]);
    }
}

/// Summary stored for a session that had nothing to analyse; same shape as
/// the summary of a finished scan.
fn empty_summary(message: &str) -> Value {
    obj! {
        "avgOverall" => Value::Null,
        "avgDj" => Value::Null,
        "avgListening" => Value::Null,
        "clipping" => 0,
        "sourceConcerns" => 0,
        "integrity" => 0,
        "attention" => 0,
        "audioSeconds" => Value::Null,
        "elapsedMs" => 0,
        "message" => message,
    }
}

/// Adds tracks to the active scan, or starts a new scan session. Tracks the
/// active session already finished are queued again.
fn enqueue(shared: &Arc<Shared>, kind: &str, ids: &[i64], roots: &[String], skipped: i64) -> Queued {
    if shared.shutdown.load(Ordering::Relaxed) {
        return Queued::default();
    }
    let mut st = lock(&shared.state);
    if st.phase == Phase::Cancelling && !ids.is_empty() {
        // The cancelled session's workers are still stopping. Starting a new
        // session now would clear the cancel flag under them and revive them.
        return Queued { refused: true, ..Queued::default() };
    }
    let active = st.scan_id.is_some() && matches!(st.phase, Phase::Running | Phase::Paused);
    if ids.is_empty() && !active {
        // Nothing to analyse: record a (finished) session so history shows it.
        if !roots.is_empty() || skipped > 0 {
            let c = shared.db.write();
            let now = now_ms();
            let roots_json = Value::Array(roots.iter().map(|r| Value::String(r.clone())).collect()).to_string_compact();
            let summary = empty_summary("No new or changed files");
            if c.execute(
                "INSERT INTO scans (kind, state, started_at, finished_at, roots, total, skipped, summary) VALUES (?, 'completed', ?, ?, ?, 0, ?, ?)",
                params![kind, now, now, roots_json, skipped, summary.to_string_compact()],
            )
            .is_ok()
            {
                let id = c.last_insert_rowid();
                drop(c);
                // The same snapshot shape as a scan that analysed files.
                let snap = obj! {
                    "id" => id,
                    "state" => "completed",
                    "kind" => kind,
                    "startedAt" => now,
                    "total" => 0,
                    "completed" => 0,
                    "failed" => 0,
                    "skipped" => skipped,
                    "cancelled" => 0,
                    "done" => 0,
                    "pending" => 0,
                    "running" => 0,
                    "current" => Value::Array(Vec::new()),
                    "elapsedMs" => 0,
                    "etaMs" => Value::Null,
                    "ratePerMin" => Value::Null,
                    "workers" => 0,
                    "indexing" => Value::Null,
                    "summary" => summary,
                };
                shared.events.emit("scan.finished", &obj! { "scan" => snap });
            }
        }
        return Queued::default();
    }
    let c = shared.db.write();
    // A new session's row is written first; it only becomes the running
    // session once its items are queued, and is dropped if none are.
    let (scan_id, new_session) = match st.scan_id.filter(|_| active) {
        Some(id) => (id, false),
        None => {
            let roots_json = Value::Array(roots.iter().map(|r| Value::String(r.clone())).collect()).to_string_compact();
            if c.execute("INSERT INTO scans (kind, state, started_at, roots, skipped) VALUES (?, 'running', ?, ?, ?)", params![kind, now_ms(), roots_json, skipped]).is_err() {
                return Queued::default();
            }
            (c.last_insert_rowid(), true)
        }
    };
    let mut added = 0i64;
    let mut already = 0i64;
    // Outcomes of finished items that go back into the queue.
    let mut requeued: Vec<String> = Vec::new();
    let mut seq = if new_session { 0 } else { st.next_seq };
    let res = c.transaction(|tx| {
        for &id in ids {
            // Only tracks that still exist (a removed one would violate the
            // foreign key, which OR IGNORE does not cover, and fail the batch).
            let n = tx.execute(
                "INSERT OR IGNORE INTO scan_items (scan_id, track_id, seq, outcome) SELECT ?, id, ?, 'queued' FROM tracks WHERE id = ?",
                params![scan_id, seq, id],
            )?;
            if n > 0 {
                seq += 1;
                added += 1;
                continue;
            }
            // Already part of this session, or no longer in the library.
            let prev = tx.query_row("SELECT outcome FROM scan_items WHERE scan_id = ? AND track_id = ?", params![scan_id, id], |r| r.text(0))?;
            match prev.as_deref() {
                None => {}
                Some("queued") | Some("running") => already += 1,
                Some(outcome) => {
                    tx.execute("UPDATE scan_items SET outcome = 'queued', seq = ?, finished_at = NULL WHERE scan_id = ? AND track_id = ?", params![seq, scan_id, id])?;
                    let col = counter_of(outcome);
                    tx.execute(&format!("UPDATE scans SET {col} = MAX(0, {col} - 1) WHERE id = ?"), params![scan_id])?;
                    seq += 1;
                    requeued.push(outcome.to_string());
                }
            }
        }
        tx.execute("UPDATE scans SET total = total + ? WHERE id = ?", params![added, scan_id])?;
        Ok::<(), StoreError>(())
    });
    if res.is_err() || (new_session && added == 0) {
        if new_session {
            let _ = c.execute("DELETE FROM scans WHERE id = ?", params![scan_id]);
        }
        drop(c);
        if let Err(e) = res {
            log_error(&format!("could not queue tracks: {e}"));
            return Queued::default();
        }
        return Queued { queued: 0, already, refused: false };
    }
    if new_session {
        shared.cancel.store(false, Ordering::Relaxed);
        st.scan_id = Some(scan_id);
        st.phase = Phase::Running;
        st.kind = kind.to_string();
        st.started_at = now_ms();
        st.started = Instant::now();
        st.total = 0;
        st.completed = 0;
        st.failed = 0;
        st.skipped = skipped;
        st.cancelled = 0;
        st.dropped = 0;
        st.recent.clear();
        st.paused_at = None;
        st.paused_total = Duration::ZERO;
    } else if skipped > 0 {
        st.skipped += skipped;
        let _ = c.execute("UPDATE scans SET skipped = skipped + ? WHERE id = ?", params![skipped, scan_id]);
    }
    drop(c);
    st.next_seq = seq;
    st.total += added;
    for outcome in &requeued {
        st.count(outcome, -1);
    }
    let queued = added + requeued.len() as i64;
    // Start workers for a new session (or top up after they drained).
    let want = shared.settings.read().unwrap_or_else(|p| p.into_inner()).workers.max(1);
    if st.workers_alive == 0 {
        st.slots = vec![None; want];
        st.alive = vec![true; want];
        for w in 0..want {
            let sh = Arc::clone(shared);
            st.workers_alive += 1;
            match std::thread::Builder::new().name(format!("sonora-worker-{w}")).spawn(move || worker(sh, w)) {
                Ok(h) => st.handles.push(h),
                Err(e) => {
                    st.workers_alive -= 1;
                    st.alive[w] = false;
                    log_error(&format!("could not start worker: {e}"));
                }
            }
        }
        let snapshot = status_json(shared, &st);
        shared.events.emit("scan.started", &obj! { "scan" => snapshot });
        if st.workers_alive == 0 {
            // No worker could start, so none would ever finish the session.
            shared.cancel.store(true, Ordering::Relaxed);
            finish_scan(shared, &mut st);
            return Queued::default();
        }
    } else {
        // Workers that found the queue empty have ended: start them again,
        // so new tracks are not left to the one still busy (B29).
        if st.slots.len() < want {
            st.slots.resize(want, None);
            st.alive.resize(want, false);
        }
        for w in 0..want {
            if st.alive.get(w).copied().unwrap_or(true) {
                continue;
            }
            let sh = Arc::clone(shared);
            match std::thread::Builder::new().name(format!("sonora-worker-{w}")).spawn(move || worker(sh, w)) {
                Ok(h) => {
                    st.handles.push(h);
                    st.alive[w] = true;
                    st.workers_alive += 1;
                }
                Err(e) => log_error(&format!("could not start worker: {e}")),
            }
        }
        shared.wake.notify_all();
    }
    emit_progress(shared, &mut st, true);
    Queued { queued, already, refused: false }
}

struct Claim {
    track_id: i64,
}

/// Claims the next queued item, waiting while paused. `None` = stop.
fn claim(shared: &Arc<Shared>, slot: usize) -> Option<Claim> {
    let mut st = lock(&shared.state);
    loop {
        if shared.cancel.load(Ordering::Relaxed) || shared.shutdown.load(Ordering::Relaxed) {
            return None;
        }
        if st.phase == Phase::Paused {
            st = shared.wake.wait_timeout(st, Duration::from_millis(500)).map(|r| r.0).unwrap_or_else(|p| p.into_inner().0);
            continue;
        }
        let scan_id = st.scan_id?;
        let c = shared.db.write();
        let next = c
            .query_row("SELECT track_id FROM scan_items WHERE scan_id = ? AND outcome = 'queued' ORDER BY seq LIMIT 1", params![scan_id], |r| r.i64(0))
            .ok()
            .flatten();
        let track_id = next?;
        let _ = c.execute("UPDATE scan_items SET outcome = 'running' WHERE scan_id = ? AND track_id = ?", params![scan_id, track_id]);
        let info = c.query_row("SELECT path, file_name FROM tracks WHERE id = ?", params![track_id], |r| (r.text(0), r.text(1))).ok().flatten();
        let _ = c.execute("UPDATE tracks SET status = ? WHERE id = ?", params![status::ANALYZING, track_id]);
        drop(c);
        let (path, file_name) = info.unwrap_or_default();
        if slot < st.slots.len() {
            st.slots[slot] = Some(Slot { track_id, file_name, path, started: Instant::now(), fraction: 0.0 });
        }
        // The table, the details panel and the track's page show it as
        // being analysed now, not only when it ends (B68).
        let row = library::get_track(&shared.db.read(), track_id).ok().flatten();
        shared.events.emit("scan.item", &obj! { "scanId" => scan_id, "trackId" => track_id, "outcome" => "running", "error" => Value::Null, "track" => row });
        emit_progress(shared, &mut st, false);
        return Some(Claim { track_id });
    }
}

fn worker(shared: Arc<Shared>, slot: usize) {
    let mut st = loop {
        while let Some(item) = claim(&shared, slot) {
            process(&shared, slot, item.track_id);
        }
        // Re-check under the state lock: `enqueue` adds items while holding
        // it, so an item queued after `claim` saw an empty queue is not lost.
        let st = lock(&shared.state);
        let stopping = shared.cancel.load(Ordering::Relaxed) || shared.shutdown.load(Ordering::Relaxed);
        let more = !stopping
            && st.scan_id.is_some_and(|id| {
                shared
                    .db
                    .read()
                    .query_row("SELECT COUNT(*) FROM scan_items WHERE scan_id = ? AND outcome = 'queued'", params![id], |r| r.i64(0))
                    .ok()
                    .flatten()
                    .unwrap_or(0)
                    > 0
            });
        if !more {
            break st;
        }
    };
    if slot < st.slots.len() {
        st.slots[slot] = None;
    }
    if let Some(a) = st.alive.get_mut(slot) {
        *a = false;
    }
    st.workers_alive = st.workers_alive.saturating_sub(1);
    if st.workers_alive == 0 {
        finish_scan(&shared, &mut st);
    }
}

fn process(shared: &Arc<Shared>, slot: usize, track_id: i64) {
    let started = Instant::now();
    let path = {
        let c = shared.db.read();
        c.query_row("SELECT path FROM tracks WHERE id = ?", params![track_id], |r| r.text(0)).ok().flatten()
    };
    let Some(path) = path else {
        record_outcome(shared, slot, track_id, "skipped", None, started);
        return;
    };
    let (opts, max_secs) = {
        let s = shared.settings.read().unwrap_or_else(|p| p.into_inner());
        let tools = shared.tools.read().unwrap_or_else(|p| p.into_inner()).clone();
        (AnalyzeOptions { tools, max_duration_secs: s.max_duration_min * 60.0, hash_file: true, ..Default::default() }, s.max_duration_min * 60.0)
    };
    let (keep_visuals, dj) = {
        let s = shared.settings.read().unwrap_or_else(|p| p.into_inner());
        (s.keep_visuals, s.dj_config())
    };
    // The file is gone (deleted, or its drive disconnected) since it was
    // queued: the track keeps its earlier analysis, and a deleted file is
    // marked missing, as a rescan would (B32).
    if !Path::new(&path).exists() {
        let deleted = Path::new(&path).parent().is_some_and(Path::is_dir);
        let _ = shared.db.write().execute(
            "UPDATE tracks SET status = CASE WHEN id IN (SELECT track_id FROM analyses) THEN 'done' ELSE 'pending' END, \
             missing_since = CASE WHEN ? THEN COALESCE(missing_since, ?) ELSE missing_since END WHERE id = ?",
            params![deleted, now_ms(), track_id],
        );
        let msg = if deleted { "the file no longer exists at this location" } else { "its folder is not available (is the drive connected?)" };
        record_outcome(shared, slot, track_id, "failed", Some(("io_error".into(), msg.into())), started);
        return;
    }
    let sh = Arc::clone(shared);
    let mut last = Instant::now() - PROGRESS_INTERVAL;
    let mut progress = move |f: f64| {
        if last.elapsed() < Duration::from_millis(100) {
            return;
        }
        last = Instant::now();
        let mut st = lock(&sh.state);
        if let Some(Some(s)) = st.slots.get_mut(slot) {
            s.fraction = f;
        }
        emit_progress(&sh, &mut st, false);
    };
    let result = {
        let cancel = &shared.cancel;
        let p = Path::new(&path);
        let mut ctl = Control { cancel, progress: &mut progress, max_duration_secs: max_secs };
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if !p.exists() {
                return Err(AnalysisError::Io("the file no longer exists at this location".into()));
            }
            sonora_engine::analyze_file(p, &opts, &mut ctl)
        }))
        .unwrap_or_else(|panic| {
            let msg = panic.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| panic.downcast_ref::<String>().cloned()).unwrap_or_else(|| "unknown panic".into());
            Err(AnalysisError::Internal(format!("analysis crashed: {msg}")))
        })
    };
    match result {
        Ok(analysis) => {
            let stored = {
                let c = shared.db.write();
                library::store_analysis(&c, track_id, &analysis, keep_visuals, &dj)
            };
            match stored {
                Ok(()) => record_outcome(shared, slot, track_id, "done", None, started),
                Err(e) => {
                    let msg = format!("could not save results: {e}");
                    let _ = library::store_failure(&shared.db.write(), track_id, "storage_error", &msg);
                    record_outcome(shared, slot, track_id, "failed", Some(("storage_error".into(), msg)), started);
                }
            }
        }
        Err(AnalysisError::Cancelled) => {
            let c = shared.db.write();
            let _ = c.execute(
                "UPDATE tracks SET status = CASE WHEN id IN (SELECT track_id FROM analyses) THEN 'done' ELSE 'pending' END WHERE id = ?",
                params![track_id],
            );
            drop(c);
            // Quitting the app interrupts (to be resumed); a user cancel does not.
            let outcome = if shared.shutdown.load(Ordering::Relaxed) { "interrupted" } else { "cancelled" };
            record_outcome(shared, slot, track_id, outcome, None, started);
        }
        Err(e) => {
            let (code, msg) = (e.code().to_string(), e.to_string());
            let _ = library::store_failure(&shared.db.write(), track_id, &code, &msg);
            crate::logging::log("warn", "analysis", &format!("{path}: {msg}"));
            record_outcome(shared, slot, track_id, "failed", Some((code, msg)), started);
        }
    }
}

fn record_outcome(shared: &Arc<Shared>, slot: usize, track_id: i64, outcome: &str, error: Option<(String, String)>, started: Instant) {
    let mut st = lock(&shared.state);
    let scan_id = st.scan_id;
    if let Some(id) = scan_id {
        let c = shared.db.write();
        let _ = c.execute("UPDATE scan_items SET outcome = ?, finished_at = ? WHERE scan_id = ? AND track_id = ?", params![outcome, now_ms(), id, track_id]);
        let col = counter_of(outcome);
        let _ = c.execute(&format!("UPDATE scans SET {col} = {col} + 1 WHERE id = ?"), params![id]);
    }
    st.count(outcome, 1);
    if outcome == "done" || outcome == "failed" {
        st.recent.push_back(started.elapsed().as_secs_f64());
        if st.recent.len() > 50 {
            st.recent.pop_front();
        }
    }
    if let Some(s) = st.slots.get_mut(slot) {
        *s = None;
    }
    let row: Option<TrackRow> = library::get_track(&shared.db.read(), track_id).ok().flatten();
    let payload = obj! {
        "scanId" => scan_id,
        "trackId" => track_id,
        "outcome" => outcome,
        "error" => error.map(|(c, m)| obj! { "code" => c, "message" => m }),
        "track" => row,
    };
    shared.events.emit("scan.item", &payload);
    emit_progress(shared, &mut st, false);
}

fn finish_scan(shared: &Arc<Shared>, st: &mut RunState) {
    let Some(scan_id) = st.scan_id else {
        st.phase = if st.pending_index_jobs > 0 { Phase::Indexing } else { Phase::Idle };
        return;
    };
    let cancelled = shared.cancel.load(Ordering::Relaxed);
    // Stopped because the app is quitting: resumable on the next launch.
    let interrupted = cancelled && shared.shutdown.load(Ordering::Relaxed);
    let state = if interrupted {
        "interrupted"
    } else if cancelled {
        "cancelled"
    } else {
        "completed"
    };
    let c = shared.db.write();
    if cancelled {
        let outcome = if interrupted { "interrupted" } else { "cancelled" };
        let n = c.execute("UPDATE scan_items SET outcome = ? WHERE scan_id = ? AND outcome = 'queued'", params![outcome, scan_id]).unwrap_or(0) as i64;
        st.cancelled += n;
        let _ = c.execute("UPDATE scans SET cancelled = cancelled + ? WHERE id = ?", params![n, scan_id]);
    }
    let agg = c
        .query_row(
            "SELECT AVG(t.overall), AVG(t.dj), AVG(t.listening), SUM(t.flags LIKE '%,clipping,%'), SUM(t.flags LIKE '%,source_concern,%'), \
             SUM(t.flags LIKE '%,integrity,%'), SUM(t.flags LIKE '%,attention,%'), SUM(t.duration) \
             FROM scan_items i JOIN tracks t ON t.id = i.track_id WHERE i.scan_id = ? AND i.outcome = 'done'",
            params![scan_id],
            |r| {
                obj! {
                    "avgOverall" => r.opt_f64(0).map(|v| num(v, 1)).unwrap_or(Value::Null),
                    "avgDj" => r.opt_f64(1).map(|v| num(v, 1)).unwrap_or(Value::Null),
                    "avgListening" => r.opt_f64(2).map(|v| num(v, 1)).unwrap_or(Value::Null),
                    "clipping" => r.opt_i64(3).unwrap_or(0),
                    "sourceConcerns" => r.opt_i64(4).unwrap_or(0),
                    "integrity" => r.opt_i64(5).unwrap_or(0),
                    "attention" => r.opt_i64(6).unwrap_or(0),
                    "audioSeconds" => r.opt_f64(7).map(|v| num(v, 0)).unwrap_or(Value::Null),
                }
            },
        )
        .ok()
        .flatten()
        .unwrap_or(Value::Null);
    // A pause still going on (cancelled while paused) is not analysis time (B69).
    let paused = st.paused_total + st.paused_at.map(|p| p.elapsed()).unwrap_or(Duration::ZERO);
    let elapsed = st.started.elapsed().saturating_sub(paused);
    let mut summary = agg;
    summary.set("elapsedMs", elapsed.as_millis() as i64);
    let _ = c.execute("UPDATE scans SET state = ?, finished_at = ?, summary = ? WHERE id = ?", params![state, now_ms(), summary.to_string_compact(), scan_id]);
    drop(c);
    st.phase = Phase::Idle;
    let mut snap = status_json(shared, st);
    snap.set("state", state);
    snap.set("summary", summary);
    st.scan_id = None;
    st.slots.clear();
    st.alive.clear();
    st.handles.retain(|h| !h.is_finished());
    shared.cancel.store(false, Ordering::Relaxed);
    if st.pending_index_jobs > 0 {
        st.phase = Phase::Indexing;
    }
    shared.events.emit("scan.finished", &obj! { "scan" => snap });
    shared.events.emit("library.changed", &obj! { "reason" => "scan_finished" });
    emit_progress(shared, st, true);
}

fn status_json(_shared: &Shared, st: &RunState) -> Value {
    let done = st.completed + st.failed + st.cancelled + st.dropped;
    let pending = (st.total - done - running_count(st) as i64).max(0);
    let elapsed = if st.scan_id.is_some() {
        let paused = st.paused_total + st.paused_at.map(|p| p.elapsed()).unwrap_or(Duration::ZERO);
        st.started.elapsed().saturating_sub(paused)
    } else {
        Duration::ZERO
    };
    let workers = st.workers_alive.max(1) as f64;
    let avg = if st.recent.is_empty() { None } else { Some(st.recent.iter().sum::<f64>() / st.recent.len() as f64) };
    let eta_ms = avg.map(|a| ((pending as f64 + running_count(st) as f64 * 0.5) * a / workers * 1000.0) as i64);
    let rate = if elapsed.as_secs_f64() > 1.0 { Some((st.completed + st.failed) as f64 / elapsed.as_secs_f64() * 60.0) } else { None };
    let current: Vec<Value> = st
        .slots
        .iter()
        .flatten()
        .map(|s| obj! { "trackId" => s.track_id, "fileName" => s.file_name, "path" => s.path, "progress" => num(s.fraction, 3), "elapsedMs" => s.started.elapsed().as_millis() as i64 })
        .collect();
    obj! {
        "id" => st.scan_id,
        "state" => st.phase.id(),
        "kind" => st.kind,
        "startedAt" => if st.scan_id.is_some() { Value::Int(st.started_at) } else { Value::Null },
        "total" => st.total,
        "completed" => st.completed,
        "failed" => st.failed,
        "skipped" => st.skipped,
        "cancelled" => st.cancelled,
        "done" => done,
        "pending" => pending,
        "running" => running_count(st),
        "current" => Value::Array(current),
        "elapsedMs" => elapsed.as_millis() as i64,
        "etaMs" => eta_ms,
        "ratePerMin" => rate.map(|r| num(r, 1)).unwrap_or(Value::Null),
        "workers" => st.workers_alive,
        "indexing" => st.indexing,
    }
}

fn running_count(st: &RunState) -> usize {
    st.slots.iter().filter(|s| s.is_some()).count()
}

fn emit_progress(shared: &Shared, st: &mut RunState, force: bool) {
    if !force && st.last_emit.elapsed() < PROGRESS_INTERVAL {
        return;
    }
    st.last_emit = Instant::now();
    let snap = status_json(shared, st);
    shared.events.emit("scan.progress", &obj! { "scan" => snap });
}

impl ToJson for Phase {
    fn to_json(&self) -> Value {
        Value::String(self.id().into())
    }
}

// ======================================= YouTube playlists from Soulseek

use crate::api::ApiError;
use crate::app::App;
use crate::matching::{self, Candidate, Want};
use crate::net::Http;
use crate::settings::PlaylistRules;
use crate::youtube;
use std::sync::Weak;

/// How often a running search is looked at.
const SEARCH_POLL: Duration = Duration::from_millis(1500);
/// After `search_seconds`, a search that slskd has not ended is stopped and
/// given this long to hand over its results.
const STOP_GRACE: Duration = Duration::from_secs(6);
/// A download that moved no bytes for this long is given up on.
const STALL: Duration = Duration::from_secs(15 * 60);
/// A download waiting in the other person's queue may wait this long.
const QUEUE_WAIT: Duration = Duration::from_secs(60 * 60);
/// A finished download that the folder watcher has not taken in this long
/// is given up on (the watcher looks every 20 seconds).
const ARRIVAL_WAIT: Duration = Duration::from_secs(10 * 60);
/// A file that arrived but has not been checked after this (its analysis
/// cancelled, say) is left as it is, with a note, so the run can end.
const CHECK_WAIT: Duration = Duration::from_secs(30 * 60);
/// Tries per track (the first pick and fallbacks).
pub const MAX_ATTEMPTS: i64 = 3;
/// Tracks not found are looked for again after this, up to `MAX_PASSES` times.
const RETRY_AFTER: Duration = Duration::from_secs(60 * 60);
const MAX_PASSES: i64 = 3;
/// Candidates kept per track (the accepted ones first).
const MAX_CANDIDATES: usize = 40;
/// Between two progress events while a search runs.
const PLAYLIST_PROGRESS: Duration = Duration::from_millis(400);

#[derive(Default)]
struct PlaylistInner {
    /// The run a worker is on.
    active: Mutex<Option<i64>>,
    /// Set (under `active`) when the run of a worker that is on its way out
    /// was started again: the worker carries on instead of ending.
    again: AtomicBool,
    cancel: AtomicBool,
    stop: AtomicBool,
    woken: Mutex<bool>,
    wake: Condvar,
    fetching: AtomicBool,
    last_emit: Mutex<Option<Instant>>,
}

/// Downloading the tracks of a YouTube playlist from Soulseek: the list is
/// fetched and read, each track searched for (one search at a time, as
/// slskd takes one), the best file by the rules downloaded, and what
/// arrives checked. Everything is in `playlist_runs` and `playlist_items`,
/// so a run quit half way carries on at the next launch.
pub struct PlaylistJobs {
    tube: Mutex<Arc<dyn Http>>,
    base: String,
    inner: Arc<PlaylistInner>,
}

impl Default for PlaylistJobs {
    fn default() -> Self {
        PlaylistJobs { tube: Mutex::new(Arc::new(crate::net::Curl::default())), base: youtube::base(), inner: Arc::new(PlaylistInner::default()) }
    }
}

fn pl_log(level: &str, msg: &str, fields: Value) {
    crate::logging::log_fields(level, "playlist", msg, fields);
}

/// One item as the database has it.
#[derive(Clone, Debug, Default)]
struct Item {
    id: i64,
    run_id: i64,
    seq: i64,
    video_id: String,
    yt_title: String,
    channel: Option<String>,
    seconds: Option<i64>,
    artist: Option<String>,
    title: String,
    mix: Option<String>,
    catalogue: Option<String>,
    included: bool,
    left_out: Option<String>,
    status: String,
    variation: Option<i64>,
    responses: Option<i64>,
    candidates: Vec<Candidate>,
    chosen: Option<i64>,
    attempts: i64,
    transfer_id: Option<String>,
    transfer_user: Option<String>,
    transfer_file: Option<String>,
    transfer_size: Option<i64>,
    bytes_seen: Option<i64>,
    progress_at: Option<i64>,
    note: Option<String>,
    error: Option<String>,
    track_id: Option<i64>,
    updated_at: i64,
}

const ITEM_COLUMNS: &str = "id, run_id, seq, video_id, yt_title, channel, seconds, artist, title, mix, catalogue, included, left_out, status, variation, responses, candidates, chosen, attempts, transfer_id, transfer_user, transfer_file, transfer_size, bytes_seen, progress_at, note, error, track_id, updated_at";

impl Item {
    fn from_row(r: &crate::db::sqlite::Row) -> Item {
        let candidates = r.opt_text(16).and_then(|t| sonora_json::parse(&t).ok()).and_then(|v| v.as_array().map(|a| a.iter().filter_map(Candidate::from_json).collect())).unwrap_or_default();
        Item {
            id: r.i64(0),
            run_id: r.i64(1),
            seq: r.i64(2),
            video_id: r.text(3),
            yt_title: r.text(4),
            channel: r.opt_text(5),
            seconds: r.opt_i64(6),
            artist: r.opt_text(7),
            title: r.text(8),
            mix: r.opt_text(9),
            catalogue: r.opt_text(10),
            included: r.i64(11) != 0,
            left_out: r.opt_text(12),
            status: r.text(13),
            variation: r.opt_i64(14),
            responses: r.opt_i64(15),
            candidates,
            chosen: r.opt_i64(17),
            attempts: r.i64(18),
            transfer_id: r.opt_text(19),
            transfer_user: r.opt_text(20),
            transfer_file: r.opt_text(21),
            transfer_size: r.opt_i64(22),
            bytes_seen: r.opt_i64(23),
            progress_at: r.opt_i64(24),
            note: r.opt_text(25),
            error: r.opt_text(26),
            track_id: r.opt_i64(27),
            updated_at: r.i64(28),
        }
    }

    fn want(&self) -> Want {
        Want { artist: self.artist.clone(), title: self.title.clone(), mix: self.mix.clone(), catalogue: self.catalogue.clone(), seconds: self.seconds }
    }

    fn accepted(&self) -> Vec<&Candidate> {
        self.candidates.iter().filter(|c| c.rejected.is_none()).collect()
    }

    fn to_json(&self, track: Option<Value>) -> Value {
        obj! {
            "id" => self.id,
            "runId" => self.run_id,
            "seq" => self.seq,
            "videoId" => self.video_id.as_str(),
            "url" => format!("https://www.youtube.com/watch?v={}", self.video_id),
            "ytTitle" => self.yt_title.as_str(),
            "channel" => self.channel.clone(),
            "seconds" => self.seconds,
            "artist" => self.artist.clone(),
            "title" => self.title.as_str(),
            "mix" => self.mix.clone(),
            "catalogue" => self.catalogue.clone(),
            "included" => self.included,
            "leftOut" => self.left_out.clone(),
            "status" => self.status.as_str(),
            "variation" => self.variation,
            "responses" => self.responses,
            "candidates" => Value::Array(self.candidates.iter().map(Candidate::to_json).collect()),
            "chosen" => self.chosen,
            "attempts" => self.attempts,
            "transfer" => self.transfer_id.as_ref().map(|id| obj! {
                "id" => id.as_str(),
                "username" => self.transfer_user.clone(),
                "filename" => self.transfer_file.clone(),
                "size" => self.transfer_size,
                "bytes" => self.bytes_seen.unwrap_or(0),
            }).unwrap_or(Value::Null),
            "note" => self.note.clone(),
            "error" => self.error.clone(),
            "trackId" => self.track_id,
            "track" => track.unwrap_or(Value::Null),
            "updatedAt" => self.updated_at,
        }
    }
}

fn load_item(c: &crate::db::sqlite::Connection, id: i64) -> Result<Option<Item>, StoreError> {
    Ok(c.query_row(&format!("SELECT {ITEM_COLUMNS} FROM playlist_items WHERE id = ?"), params![id], Item::from_row)?)
}

fn load_items(c: &crate::db::sqlite::Connection, run_id: i64, cond: &str) -> Result<Vec<Item>, StoreError> {
    Ok(c.query(&format!("SELECT {ITEM_COLUMNS} FROM playlist_items WHERE run_id = ? AND {cond} ORDER BY seq"), params![run_id], Item::from_row)?)
}

fn candidates_json(c: &[Candidate]) -> String {
    Value::Array(c.iter().take(MAX_CANDIDATES).map(Candidate::to_json).collect()).to_string_compact()
}

/// A run's row and the counts of its items.
fn run_json(c: &crate::db::sqlite::Connection, id: i64) -> Result<Option<Value>, StoreError> {
    let Some(mut v) = c.query_row(
        "SELECT id, created_at, updated_at, state, source, playlist_id, title, rules, confirmed, passes, retry_at, error, finished_at FROM playlist_runs WHERE id = ?",
        params![id],
        |r| {
            obj! {
                "id" => r.i64(0),
                "createdAt" => r.i64(1),
                "updatedAt" => r.i64(2),
                "state" => r.text(3),
                "source" => r.text(4),
                "playlistId" => r.opt_text(5),
                "title" => r.opt_text(6),
                "rules" => r.opt_text(7).and_then(|t| sonora_json::parse(&t).ok()).unwrap_or(Value::Null),
                "confirmed" => r.i64(8) != 0,
                "passes" => r.i64(9),
                "retryAt" => r.opt_i64(10),
                "error" => r.opt_text(11),
                "finishedAt" => r.opt_i64(12),
            }
        },
    )?
    else {
        return Ok(None);
    };
    let mut counts = obj! { "total" => 0, "included" => 0, "leftOut" => 0, "pending" => 0, "searching" => 0, "found" => 0, "notFound" => 0, "downloading" => 0, "verifying" => 0, "done" => 0, "failed" => 0 };
    let rows = c.query("SELECT included, status, COUNT(*) FROM playlist_items WHERE run_id = ? GROUP BY included, status", params![id], |r| (r.i64(0) != 0, r.text(1), r.i64(2)))?;
    let mut total = 0;
    let mut included = 0;
    for (inc, status, n) in rows {
        total += n;
        if !inc {
            continue;
        }
        included += n;
        let key = match status.as_str() {
            "pending" => "pending",
            "searching" => "searching",
            "found" => "found",
            "not_found" => "notFound",
            "downloading" => "downloading",
            "verifying" => "verifying",
            "done" => "done",
            "failed" => "failed",
            _ => continue,
        };
        let prev = counts.get(key).and_then(Value::as_i64).unwrap_or(0);
        counts.set(key, prev + n);
    }
    counts.set("total", total);
    counts.set("included", included);
    counts.set("leftOut", total - included);
    v.set("counts", counts);
    let current = c.query_row("SELECT id, artist, title, variation FROM playlist_items WHERE run_id = ? AND status = 'searching' ORDER BY seq LIMIT 1", params![id], |r| {
        obj! { "itemId" => r.i64(0), "title" => match r.opt_text(1) { Some(a) => format!("{a} – {}", r.text(2)), None => r.text(2) }, "variation" => r.opt_i64(3) }
    })?;
    v.set("current", current.unwrap_or(Value::Null));
    Ok(Some(v))
}

impl PlaylistJobs {
    /// Replaces how YouTube is reached (tests).
    pub fn set_http(&self, h: Arc<dyn Http>) {
        *lock(&self.tube) = h;
    }

    fn emit_run(&self, app: &App, run_id: i64, force: bool) {
        if !force {
            let mut last = lock(&self.inner.last_emit);
            if last.is_some_and(|t| t.elapsed() < PLAYLIST_PROGRESS) {
                return;
            }
            *last = Some(Instant::now());
        }
        if let Ok(Some(v)) = run_json(&app.db.read(), run_id) {
            app.events.emit("playlist.changed", &v);
        }
    }

    fn emit_item(&self, app: &App, item_id: i64) {
        let c = app.db.read();
        if let Ok(Some(item)) = load_item(&c, item_id) {
            let track = item.track_id.and_then(|id| library::get_track(&c, id).ok().flatten()).map(|t| t.to_json());
            drop(c);
            app.events.emit("playlist.item", &item.to_json(track));
        }
    }

    // ---------------------------------------------------------- the list

    /// Starts reading the playlist at `source` (a link or an id) in the
    /// background: a run in state `fetching`, then `draft` with its items,
    /// or `failed` with the reason. The run's id.
    pub fn fetch(&self, app: &Arc<App>, source: &str) -> Result<i64, ApiError> {
        let id = youtube::playlist_id(source).map_err(|m| ApiError::new("invalid_params", m))?;
        if self.inner.fetching.swap(true, Ordering::SeqCst) {
            return Err(ApiError::new("busy", "A playlist is being read already; wait a moment."));
        }
        let now = now_ms();
        let run_id = {
            let c = app.db.write();
            let inserted = c.execute("INSERT INTO playlist_runs (created_at, updated_at, state, source, playlist_id) VALUES (?, ?, 'fetching', ?, ?)", params![now, now, source.trim(), id]);
            if let Err(e) = inserted {
                self.inner.fetching.store(false, Ordering::SeqCst);
                return Err(e.into());
            }
            c.last_insert_rowid()
        };
        let weak: Weak<App> = Arc::downgrade(app);
        let tube = Arc::clone(&lock(&self.tube));
        let base = self.base.clone();
        let inner = Arc::clone(&self.inner);
        let spawned = std::thread::Builder::new().name("sonora-playlist-fetch".into()).spawn(move || {
            let Some(app) = weak.upgrade() else { return };
            let mut last = Instant::now();
            let progress = |n: usize| {
                if last.elapsed() > Duration::from_millis(300) {
                    last = Instant::now();
                    app.events.emit("playlist.changed", &obj! { "id" => run_id, "state" => "fetching", "videos" => n as i64 });
                }
            };
            let mut progress = progress;
            let result = youtube::fetch(tube.as_ref(), &base, &id, &inner.stop, &mut progress);
            let now = now_ms();
            match result {
                Ok(list) => {
                    let c = app.db.write();
                    let res = c.transaction(|tx| {
                        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
                        for (i, v) in list.videos.iter().enumerate() {
                            let p = youtube::parse_title(&v.title, &v.channel);
                            let mut left_out = youtube::leave_out(v, &p);
                            let key = youtube::track_key(&p);
                            if left_out.is_none() && !seen.insert(key) {
                                left_out = Some("The same track is in the list already.".into());
                            }
                            tx.execute(
                                "INSERT INTO playlist_items (run_id, seq, video_id, yt_title, channel, seconds, artist, title, mix, catalogue, included, left_out, status, updated_at) \
                                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'pending', ?)",
                                params![run_id, i as i64, v.id, v.title, v.channel, v.seconds, p.artist, p.title, p.mix, p.catalogue, left_out.is_none(), left_out, now],
                            )?;
                        }
                        let error: Option<String> = list.truncated.then(|| format!("Only the first {} videos were read.", youtube::MAX_VIDEOS));
                        tx.execute("UPDATE playlist_runs SET state = 'draft', title = ?, error = ?, updated_at = ? WHERE id = ?", params![list.title, error, now, run_id])?;
                        Ok::<(), StoreError>(())
                    });
                    if let Err(e) = res {
                        let _ = c.execute("UPDATE playlist_runs SET state = 'failed', error = ?, updated_at = ? WHERE id = ?", params![format!("The list could not be saved: {e}"), now, run_id]);
                    }
                    pl_log("info", "playlist read", obj! { "run" => run_id, "videos" => list.videos.len() as i64 });
                }
                Err(e) => {
                    let msg = e.to_string();
                    let _ = app.db.write().execute("UPDATE playlist_runs SET state = 'failed', error = ?, updated_at = ? WHERE id = ?", params![msg.clone(), now, run_id]);
                    pl_log("warn", "playlist could not be read", obj! { "run" => run_id, "code" => e.code() });
                }
            }
            inner.fetching.store(false, Ordering::SeqCst);
            let summary = run_json(&app.db.read(), run_id).ok().flatten();
            if let Some(v) = summary {
                app.events.emit("playlist.changed", &v);
            }
        });
        if spawned.is_err() {
            self.inner.fetching.store(false, Ordering::SeqCst);
            let _ = app.db.write().execute("UPDATE playlist_runs SET state = 'failed', error = 'SONORA could not start reading the playlist.' WHERE id = ?", params![run_id]);
        }
        Ok(run_id)
    }

    /// Every run, newest first (without their items).
    pub fn list(&self, app: &App) -> Result<Value, StoreError> {
        let c = app.db.read();
        let ids = c.query("SELECT id FROM playlist_runs ORDER BY id DESC LIMIT 100", params![], |r| r.i64(0))?;
        let mut out = Vec::new();
        for id in ids {
            if let Some(v) = run_json(&c, id)? {
                out.push(v);
            }
        }
        Ok(Value::Array(out))
    }

    /// One run with its items (and the library track of each finished one).
    pub fn get(&self, app: &App, id: i64) -> Result<Value, ApiError> {
        let c = app.db.read();
        let mut run = run_json(&c, id)?.ok_or_else(|| ApiError::new("not_found", "That playlist run is gone."))?;
        let items = load_items(&c, id, "1")?;
        let ids: Vec<i64> = items.iter().filter_map(|i| i.track_id).collect();
        let tracks: std::collections::HashMap<i64, Value> = library::get_tracks(&c, &ids)?.into_iter().map(|t| (t.id, t.to_json())).collect();
        run.set("items", Value::Array(items.iter().map(|i| i.to_json(i.track_id.and_then(|t| tracks.get(&t).cloned()))).collect()));
        Ok(run)
    }

    /// Removes a run and everything noted about it. Not while it runs.
    pub fn delete(&self, app: &App, id: i64) -> Result<(), ApiError> {
        let active = lock(&self.inner.active);
        if *active == Some(id) {
            // A run waiting to look again has nothing on its way: its worker
            // only sleeps, and ends when it finds the run gone.
            let c = app.db.write();
            let state = c.query_row("SELECT state FROM playlist_runs WHERE id = ?", params![id], |r| r.text(0))?;
            if state.as_deref() != Some("waiting") {
                return Err(ApiError::new("busy", "Stop the run before removing it."));
            }
            c.execute("DELETE FROM playlist_runs WHERE id = ?", params![id])?;
            drop(c);
            drop(active);
            self.wake();
            return Ok(());
        }
        app.db.write().execute("DELETE FROM playlist_runs WHERE id = ?", params![id])?;
        Ok(())
    }

    /// Refuses (before anything is changed) when a worker is on another
    /// run. The caller keeps the guard until it has spawned.
    fn claim(&self, run_id: i64) -> Result<MutexGuard<'_, Option<i64>>, ApiError> {
        let active = lock(&self.inner.active);
        match *active {
            Some(other) if other != run_id => Err(ApiError::new("busy", "Another playlist is being searched. Wait for it to finish, or stop it first.")),
            _ => Ok(active),
        }
    }

    /// Puts videos in or out of the tracks to look for (a draft, or a run
    /// that is not searching them yet).
    pub fn include(&self, app: &App, run_id: i64, item_ids: &[i64], included: bool) -> Result<Value, ApiError> {
        {
            let c = app.db.write();
            let now = now_ms();
            for id in item_ids {
                c.execute(
                    "UPDATE playlist_items SET included = ?, updated_at = ? WHERE id = ? AND run_id = ? AND status IN ('pending', 'not_found', 'failed')",
                    params![included, now, *id, run_id],
                )?;
            }
        }
        for id in item_ids {
            self.emit_item(app, *id);
        }
        Ok(run_json(&app.db.read(), run_id)?.unwrap_or(Value::Null))
    }

    // ------------------------------------------------------- the search

    /// Starts searching and downloading a draft, with the rules in use now
    /// (kept with the run). One run searches at a time.
    pub fn start(&self, app: &Arc<App>, run_id: i64) -> Result<Value, ApiError> {
        let rules = app.settings().playlist_rules;
        let mut active = self.claim(run_id)?;
        {
            let c = app.db.write();
            let state = c.query_row("SELECT state FROM playlist_runs WHERE id = ?", params![run_id], |r| r.text(0))?.ok_or_else(|| ApiError::new("not_found", "That playlist run is gone."))?;
            if !matches!(state.as_str(), "draft" | "finished" | "cancelled" | "waiting" | "review") {
                return Err(ApiError::new("invalid_params", format!("This run is {state}; it cannot be started now.")));
            }
            let n = c.query_row("SELECT COUNT(*) FROM playlist_items WHERE run_id = ? AND included = 1 AND status IN ('pending', 'not_found')", params![run_id], |r| r.i64(0))?.unwrap_or(0);
            if n == 0 && state == "draft" {
                return Err(ApiError::new("invalid_params", "There is nothing to look for: every video is left out."));
            }
            let now = now_ms();
            c.execute(
                "UPDATE playlist_runs SET state = 'running', rules = ?, confirmed = ?, error = NULL, retry_at = NULL, finished_at = NULL, updated_at = ? WHERE id = ?",
                params![rules.to_json().to_string_compact(), rules.mode == "automatic", now, run_id],
            )?;
            // Searched again from the start: what was not found gets another look.
            c.execute("UPDATE playlist_items SET status = 'pending', updated_at = ? WHERE run_id = ? AND included = 1 AND status = 'not_found'", params![now, run_id])?;
        }
        self.spawn_locked(&mut active, app, run_id)?;
        drop(active);
        self.wake();
        self.emit_run(app, run_id, true);
        Ok(run_json(&app.db.read(), run_id)?.unwrap_or(Value::Null))
    }

    /// Resumes runs left running when SONORA last quit.
    pub fn resume(&self, app: &Arc<App>) {
        let runs = {
            let c = app.db.write();
            let now = now_ms();
            let _ = c.execute("UPDATE playlist_runs SET state = 'failed', error = 'SONORA quit while the list was being read. Fetch it again.', updated_at = ? WHERE state = 'fetching'", params![now]);
            let _ = c.execute("UPDATE playlist_items SET status = 'pending', updated_at = ? WHERE status = 'searching'", params![now]);
            c.query("SELECT id FROM playlist_runs WHERE state IN ('running', 'paused', 'waiting') ORDER BY id", params![], |r| r.i64(0)).unwrap_or_default()
        };
        for id in runs {
            if self.spawn(app, id).is_ok() {
                pl_log("info", "resumed a playlist run", obj! { "run" => id });
                break;
            }
        }
    }

    fn spawn(&self, app: &Arc<App>, run_id: i64) -> Result<(), ApiError> {
        let mut active = self.claim(run_id)?;
        self.spawn_locked(&mut active, app, run_id)
    }

    fn spawn_locked(&self, active: &mut MutexGuard<'_, Option<i64>>, app: &Arc<App>, run_id: i64) -> Result<(), ApiError> {
        if active.is_some() {
            // The worker is on this run already; if it is about to end, it
            // looks at the run once more.
            self.inner.again.store(true, Ordering::SeqCst);
            return Ok(());
        }
        self.inner.cancel.store(false, Ordering::SeqCst);
        self.inner.again.store(false, Ordering::SeqCst);
        let weak: Weak<App> = Arc::downgrade(app);
        let inner = Arc::clone(&self.inner);
        let spawned = std::thread::Builder::new().name("sonora-playlist".into()).spawn(move || {
            let worker = Worker { inner: Arc::clone(&inner), run_id };
            loop {
                worker.run(weak.clone());
                let mut active = lock(&inner.active);
                if inner.again.swap(false, Ordering::SeqCst) && !inner.stop.load(Ordering::SeqCst) && !inner.cancel.load(Ordering::SeqCst) {
                    continue;
                }
                *active = None;
                break;
            }
        });
        if spawned.is_err() {
            return Err(ApiError::new("internal", "SONORA could not start the playlist worker."));
        }
        **active = Some(run_id);
        Ok(())
    }

    fn wake(&self) {
        *lock(&self.inner.woken) = true;
        self.inner.wake.notify_all();
    }

    pub fn pause(&self, app: &App, run_id: i64) -> Result<Value, ApiError> {
        let n = app.db.write().execute("UPDATE playlist_runs SET state = 'paused', updated_at = ? WHERE id = ? AND state = 'running'", params![now_ms(), run_id])?;
        if n == 0 {
            return Err(ApiError::new("invalid_params", "Only a running playlist can be paused."));
        }
        self.wake();
        self.emit_run(app, run_id, true);
        Ok(run_json(&app.db.read(), run_id)?.unwrap_or(Value::Null))
    }

    /// Carries on with a paused run, with the rules in use now (changed
    /// while it was paused, say): they apply from the next search on.
    pub fn resume_run(&self, app: &Arc<App>, run_id: i64) -> Result<Value, ApiError> {
        let rules = app.settings().playlist_rules;
        let mut active = self.claim(run_id)?;
        let n = app.db.write().execute(
            "UPDATE playlist_runs SET state = 'running', rules = ?, confirmed = (confirmed OR ?), retry_at = NULL, updated_at = ? WHERE id = ? AND state IN ('paused', 'waiting')",
            params![rules.to_json().to_string_compact(), rules.mode == "automatic", now_ms(), run_id],
        )?;
        if n == 0 {
            return Err(ApiError::new("invalid_params", "Only a paused playlist can be resumed."));
        }
        self.spawn_locked(&mut active, app, run_id)?;
        drop(active);
        self.wake();
        self.emit_run(app, run_id, true);
        Ok(run_json(&app.db.read(), run_id)?.unwrap_or(Value::Null))
    }

    /// Stops a run: searching stops, downloads on their way are cancelled
    /// in slskd, finished tracks stay.
    pub fn cancel(&self, app: &App, run_id: i64) -> Result<Value, ApiError> {
        let active = *lock(&self.inner.active);
        if active == Some(run_id) {
            self.inner.cancel.store(true, Ordering::SeqCst);
            self.wake();
        } else {
            // Nothing is working on it (a run in review or waiting): close it here.
            let c = app.db.write();
            let now = now_ms();
            c.execute("UPDATE playlist_runs SET state = 'cancelled', finished_at = ?, updated_at = ? WHERE id = ? AND state IN ('running', 'paused', 'review', 'waiting')", params![now, now, run_id])?;
            c.execute("UPDATE playlist_items SET status = 'pending', updated_at = ? WHERE run_id = ? AND status IN ('searching', 'found')", params![now, run_id])?;
        }
        self.emit_run(app, run_id, true);
        Ok(run_json(&app.db.read(), run_id)?.unwrap_or(Value::Null))
    }

    /// In review mode: the picks are downloaded.
    pub fn confirm(&self, app: &Arc<App>, run_id: i64) -> Result<Value, ApiError> {
        let now = now_ms();
        let mut active = self.claim(run_id)?;
        let n = app.db.write().execute("UPDATE playlist_runs SET confirmed = 1, state = 'running', updated_at = ? WHERE id = ? AND state IN ('review', 'running', 'paused')", params![now, run_id])?;
        if n == 0 {
            return Err(ApiError::new("invalid_params", "This run has no picks waiting."));
        }
        self.spawn_locked(&mut active, app, run_id)?;
        drop(active);
        self.wake();
        self.emit_run(app, run_id, true);
        Ok(run_json(&app.db.read(), run_id)?.unwrap_or(Value::Null))
    }

    /// Picks another of a track's candidates. A download on its way is
    /// cancelled; the new pick is queued when the run next looks.
    pub fn choose(&self, app: &Arc<App>, run_id: i64, item_id: i64, candidate: i64) -> Result<Value, ApiError> {
        let item = load_item(&app.db.read(), item_id)?.filter(|i| i.run_id == run_id).ok_or_else(|| ApiError::new("not_found", "That track is not in this run."))?;
        let accepted = item.accepted().len() as i64;
        if candidate < 0 || candidate >= item.candidates.len() as i64 {
            return Err(ApiError::new("invalid_params", "That candidate is not in the list."));
        }
        if candidate >= accepted {
            return Err(ApiError::new("invalid_params", "That file was rejected by the rules; choose one that passed."));
        }
        // Refused before anything is cancelled when another run is working.
        drop(self.claim(run_id)?);
        if matches!(item.status.as_str(), "downloading") {
            if let (Some(u), Some(t)) = (&item.transfer_user, &item.transfer_id) {
                if let Err(e) = app.slskd.cancel_transfer(app, u, t, false) {
                    pl_log("warn", "could not cancel the earlier pick", obj! { "code" => e.code });
                }
            }
        }
        let pick = &item.candidates[candidate as usize];
        let note = format!("You chose {} from {}.", pick.name, pick.username);
        let now = now_ms();
        let mut active = self.claim(run_id)?;
        {
            let c = app.db.write();
            c.execute(
                "UPDATE playlist_items SET chosen = ?, status = 'found', transfer_id = NULL, transfer_user = NULL, transfer_file = NULL, transfer_size = NULL, bytes_seen = NULL, progress_at = NULL, error = NULL, note = ?, attempts = 0, updated_at = ? WHERE id = ?",
                params![candidate, note, now, item_id],
            )?;
            // A finished run takes the new pick up again.
            c.execute("UPDATE playlist_runs SET state = 'running', confirmed = 1, finished_at = NULL, updated_at = ? WHERE id = ? AND state IN ('finished', 'cancelled', 'waiting')", params![now, run_id])?;
        }
        self.spawn_locked(&mut active, app, run_id)?;
        drop(active);
        self.wake();
        self.emit_item(app, item_id);
        self.emit_run(app, run_id, true);
        Ok(run_json(&app.db.read(), run_id)?.unwrap_or(Value::Null))
    }

    /// Looks again for tracks that were not found (all of them, or `item_ids`).
    pub fn retry(&self, app: &Arc<App>, run_id: i64, item_ids: Option<&[i64]>) -> Result<Value, ApiError> {
        let now = now_ms();
        let mut active = self.claim(run_id)?;
        {
            let c = app.db.write();
            let n = match item_ids {
                Some(ids) => {
                    let mut n = 0;
                    for id in ids {
                        n += c.execute("UPDATE playlist_items SET status = 'pending', included = 1, error = NULL, attempts = 0, updated_at = ? WHERE id = ? AND run_id = ? AND status IN ('not_found', 'failed')", params![now, *id, run_id])?;
                    }
                    n
                }
                None => c.execute("UPDATE playlist_items SET status = 'pending', error = NULL, attempts = 0, updated_at = ? WHERE run_id = ? AND included = 1 AND status IN ('not_found', 'failed')", params![now, run_id])?,
            };
            if n == 0 {
                return Err(ApiError::new("invalid_params", "There is nothing to look for again."));
            }
            c.execute("UPDATE playlist_runs SET state = 'running', retry_at = NULL, finished_at = NULL, updated_at = ? WHERE id = ? AND state IN ('finished', 'cancelled', 'waiting', 'review')", params![now, run_id])?;
        }
        self.spawn_locked(&mut active, app, run_id)?;
        drop(active);
        self.wake();
        self.emit_run(app, run_id, true);
        Ok(run_json(&app.db.read(), run_id)?.unwrap_or(Value::Null))
    }

    /// Stops the worker on the way out; the run stays as it is and carries
    /// on at the next launch.
    pub fn shutdown(&self) {
        self.inner.stop.store(true, Ordering::SeqCst);
        self.wake();
    }

    pub fn is_active(&self) -> Option<i64> {
        *lock(&self.inner.active)
    }
}

/// Whether an item read a moment ago (`seen`) is still on the same pick
/// as it is now: same status, same file asked for, same candidate.
fn still_the_pick(seen: &Item, now: &Item) -> bool {
    seen.status == now.status && seen.transfer_file == now.transfer_file && seen.chosen == now.chosen
}

/// What happens to one item in one tick.
enum Tick {
    Done,
    Wait,
}

struct Worker {
    inner: Arc<PlaylistInner>,
    run_id: i64,
}

impl Worker {
    fn stopping(&self) -> bool {
        self.inner.stop.load(Ordering::Relaxed) || self.inner.cancel.load(Ordering::Relaxed)
    }

    /// Sleeps at most `d`, less when woken.
    fn nap(&self, d: Duration) {
        let mut woken = lock(&self.inner.woken);
        if !*woken {
            woken = self.inner.wake.wait_timeout(woken, d).map(|r| r.0).unwrap_or_else(|p| p.into_inner().0);
        }
        *woken = false;
    }

    fn rules(&self, app: &App) -> PlaylistRules {
        let text = app.db.read().query_row("SELECT rules FROM playlist_runs WHERE id = ?", params![self.run_id], |r| r.opt_text(0)).ok().flatten().flatten();
        text.and_then(|t| sonora_json::parse(&t).ok()).and_then(|v| PlaylistRules::from_json(&v).ok()).unwrap_or_else(|| app.settings().playlist_rules)
    }

    fn state(&self, app: &App) -> (String, bool, i64, Option<i64>) {
        app.db
            .read()
            .query_row("SELECT state, confirmed, passes, retry_at FROM playlist_runs WHERE id = ?", params![self.run_id], |r| (r.text(0), r.i64(1) != 0, r.i64(2), r.opt_i64(3)))
            .ok()
            .flatten()
            .unwrap_or_else(|| ("cancelled".into(), false, 0, None))
    }

    fn set_state(&self, app: &App, state: &str, error: Option<&str>) {
        let now = now_ms();
        let finished = matches!(state, "finished" | "cancelled" | "failed").then_some(now);
        let _ = app.db.write().execute("UPDATE playlist_runs SET state = ?, error = ?, finished_at = ?, updated_at = ? WHERE id = ?", params![state, error, finished, now, self.run_id]);
    }

    fn run(&self, weak: Weak<App>) {
        let mut slskd_down: Option<Instant> = None;
        // The track whose search keeps failing for another reason than slskd
        // being away, and how often in a row (B107).
        let mut failing: (i64, u32) = (0, 0);
        loop {
            let Some(app) = weak.upgrade() else { return };
            if self.inner.stop.load(Ordering::Relaxed) {
                return;
            }
            if self.inner.cancel.load(Ordering::Relaxed) {
                self.cancel_all(&app);
                return;
            }
            let (state, confirmed, passes, retry_at) = self.state(&app);
            let rules = self.rules(&app);
            match state.as_str() {
                "running" => {}
                "paused" => {
                    self.tick_downloads(&app, &rules);
                    drop(app);
                    self.nap(Duration::from_secs(1));
                    continue;
                }
                "waiting" => {
                    let due = retry_at.is_none_or(|t| now_ms() >= t);
                    if due {
                        let now = now_ms();
                        let c = app.db.write();
                        let _ = c.execute("UPDATE playlist_items SET status = 'pending', updated_at = ? WHERE run_id = ? AND included = 1 AND status = 'not_found'", params![now, self.run_id]);
                        let _ = c.execute("UPDATE playlist_runs SET state = 'running', retry_at = NULL, passes = passes + 1, updated_at = ? WHERE id = ?", params![now, self.run_id]);
                        drop(c);
                        app.playlist.emit_run(&app, self.run_id, true);
                    } else {
                        drop(app);
                        self.nap(Duration::from_secs(30));
                    }
                    continue;
                }
                _ => return,
            }
            self.tick_downloads(&app, &rules);
            if confirmed {
                self.start_downloads(&app, &rules);
            }
            let next = load_items(&app.db.read(), self.run_id, "included = 1 AND status = 'pending'").ok().and_then(|v| v.into_iter().next());
            if let Some(item) = next {
                match self.search(&app, &item, &rules) {
                    Ok(()) => {
                        slskd_down = None;
                        // slskd answers again: the earlier "could not reach" goes.
                        let cleared = app.db.write().execute("UPDATE playlist_runs SET error = NULL WHERE id = ? AND state = 'running' AND error IS NOT NULL", params![self.run_id]).unwrap_or(0);
                        if cleared > 0 {
                            app.playlist.emit_run(&app, self.run_id, true);
                        }
                    }
                    Err(e) if {
                        // slskd away or not set up: wait for it, however long.
                        // An error of slskd's own (it may be starting): skip
                        // after ten in a row. Anything else: after three.
                        let limit = match e.code {
                            "slskd_unreachable" | "slskd_offline" | "slskd_busy" | "slskd_unauthorized" | "slskd_forbidden" | "slskd_not_set_up" | "slskd_too_old" | "secret_store" => u32::MAX,
                            "slskd_failed" => 10,
                            _ => 3,
                        };
                        failing = if failing.0 == item.id { (item.id, failing.1 + 1) } else { (item.id, 1) };
                        failing.1 >= limit
                    } =>
                    {
                        // Not slskd being away: this track's search fails. It is
                        // skipped, with the reason, and the run goes on.
                        let now = now_ms();
                        let _ = app.db.write().execute(
                            "UPDATE playlist_items SET status = 'not_found', error = ?, updated_at = ? WHERE id = ?",
                            params![format!("Skipped: the search kept failing ({}).", e.message), now, item.id],
                        );
                        let _ = app.db.write().execute("UPDATE playlist_runs SET error = NULL, updated_at = ? WHERE id = ?", params![now, self.run_id]);
                        pl_log("warn", "a track's search kept failing; skipped", obj! { "run" => self.run_id, "item" => item.id, "code" => e.code });
                        failing = (0, 0);
                        app.playlist.emit_item(&app, item.id);
                        app.playlist.emit_run(&app, self.run_id, true);
                    }
                    Err(e) => {
                        // slskd is not answering: say so, and try the same track again in a while.
                        let now = now_ms();
                        let _ = app.db.write().execute("UPDATE playlist_items SET status = 'pending', updated_at = ? WHERE id = ?", params![now, item.id]);
                        let _ = app.db.write().execute("UPDATE playlist_runs SET error = ?, updated_at = ? WHERE id = ?", params![e.message.clone(), now, self.run_id]);
                        if slskd_down.is_none() {
                            pl_log("warn", "search failed; waiting for slskd", obj! { "run" => self.run_id, "code" => e.code });
                        }
                        slskd_down.get_or_insert_with(Instant::now);
                        app.playlist.emit_run(&app, self.run_id, true);
                        drop(app);
                        self.nap(Duration::from_secs(15));
                    }
                }
                continue;
            }
            let active = app
                .db
                .read()
                .query_row("SELECT COUNT(*) FROM playlist_items WHERE run_id = ? AND status IN ('downloading', 'verifying', 'found')", params![self.run_id], |r| r.i64(0))
                .ok()
                .flatten()
                .unwrap_or(0);
            if active > 0 && (confirmed || rules.mode == "automatic") {
                drop(app);
                self.nap(Duration::from_secs(2));
                continue;
            }
            if !confirmed {
                let found = app.db.read().query_row("SELECT COUNT(*) FROM playlist_items WHERE run_id = ? AND status = 'found'", params![self.run_id], |r| r.i64(0)).ok().flatten().unwrap_or(0);
                if found > 0 {
                    self.set_state(&app, "review", None);
                    app.playlist.emit_run(&app, self.run_id, true);
                    return;
                }
            }
            let not_found = app.db.read().query_row("SELECT COUNT(*) FROM playlist_items WHERE run_id = ? AND included = 1 AND status = 'not_found'", params![self.run_id], |r| r.i64(0)).ok().flatten().unwrap_or(0);
            if not_found > 0 && rules.retry_not_found && passes < MAX_PASSES {
                let at = now_ms() + RETRY_AFTER.as_millis() as i64;
                let _ = app.db.write().execute("UPDATE playlist_runs SET state = 'waiting', retry_at = ?, updated_at = ? WHERE id = ?", params![at, now_ms(), self.run_id]);
                app.playlist.emit_run(&app, self.run_id, true);
                drop(app);
                self.nap(Duration::from_secs(30));
                continue;
            }
            self.set_state(&app, "finished", None);
            let summary = run_json(&app.db.read(), self.run_id).ok().flatten().unwrap_or(Value::Null);
            pl_log("info", "playlist run finished", obj! { "run" => self.run_id });
            app.events.emit("playlist.finished", &summary);
            app.events.emit("playlist.changed", &summary);
            return;
        }
    }

    /// Stops everything on the way: downloads are cancelled in slskd, the
    /// run is marked cancelled. Finished tracks stay.
    fn cancel_all(&self, app: &Arc<App>) {
        let items = load_items(&app.db.read(), self.run_id, "status IN ('downloading', 'verifying', 'searching', 'found')").unwrap_or_default();
        let now = now_ms();
        for item in items {
            if item.status == "downloading" {
                if let (Some(u), Some(t)) = (&item.transfer_user, &item.transfer_id) {
                    let _ = app.slskd.cancel_transfer(app, u, t, false);
                }
                let _ = app.db.write().execute("UPDATE playlist_items SET status = 'failed', error = 'Stopped.', updated_at = ? WHERE id = ?", params![now, item.id]);
            } else {
                let _ = app.db.write().execute("UPDATE playlist_items SET status = 'pending', updated_at = ? WHERE id = ?", params![now, item.id]);
            }
            app.playlist.emit_item(app, item.id);
        }
        self.set_state(app, "cancelled", None);
        let summary = run_json(&app.db.read(), self.run_id).ok().flatten().unwrap_or(Value::Null);
        app.events.emit("playlist.finished", &summary);
        app.events.emit("playlist.changed", &summary);
    }

    /// Looks for one track: the search variations in turn, until one finds
    /// a file that passes the rules.
    fn search(&self, app: &Arc<App>, item: &Item, rules: &PlaylistRules) -> Result<(), crate::slskd::SlskdError> {
        let now = now_ms();
        let db_err = |e: crate::db::sqlite::DbError| crate::slskd::SlskdError::new("storage_error", e.to_string());
        app.db.write().execute("UPDATE playlist_items SET status = 'searching', variation = NULL, updated_at = ? WHERE id = ?", params![now, item.id]).map_err(db_err)?;
        app.playlist.emit_item(app, item.id);
        app.playlist.emit_run(app, self.run_id, true);
        let want = item.want();
        let mut best: Vec<Candidate> = Vec::new();
        let mut responses = 0i64;
        let mut used = None;
        // Searches slskd refused (B107): skipped, never retried.
        let mut refused: Option<String> = None;
        let mut made = 0;
        for (n, text) in matching::variations(&want) {
            if self.stopping() {
                return Ok(());
            }
            if self.state(app).0 != "running" {
                let _ = app.db.write().execute("UPDATE playlist_items SET status = 'pending', updated_at = ? WHERE id = ?", params![now_ms(), item.id]);
                app.playlist.emit_item(app, item.id);
                return Ok(());
            }
            let _ = app.db.write().execute("UPDATE playlist_items SET variation = ?, updated_at = ? WHERE id = ?", params![n as i64, now_ms(), item.id]);
            app.playlist.emit_run(app, self.run_id, true);
            let id = match app.slskd.search(app, &text) {
                Ok(v) => {
                    made += 1;
                    v.get("id").and_then(Value::as_str).unwrap_or("").to_string()
                }
                Err(e) if e.code == "slskd_rejected" => {
                    pl_log("warn", "slskd refused a search; trying the next", obj! { "run" => self.run_id, "item" => item.id, "variation" => n as i64 });
                    refused = Some(e.message);
                    continue;
                }
                Err(e) => return Err(e),
            };
            let started = Instant::now();
            let mut stopped = false;
            let st = loop {
                self.nap(SEARCH_POLL);
                if self.stopping() {
                    let _ = app.slskd.stop(app, &id);
                    return Ok(());
                }
                self.tick_downloads(app, rules);
                let st = app.slskd.search_state(app, &id, false)?;
                if st.state != "searching" {
                    break st;
                }
                let limit = Duration::from_secs(rules.search_seconds.clamp(15, 25) as u64);
                if !stopped && started.elapsed() > limit {
                    let _ = app.slskd.stop(app, &id);
                    stopped = true;
                } else if stopped && started.elapsed() > limit + STOP_GRACE {
                    break st;
                }
                app.playlist.emit_run(app, self.run_id, false);
            };
            responses += st.responses;
            let ranked = matching::rank(&st.folders, &want, rules);
            let ok = matching::sufficient(&ranked);
            if ok || ranked.len() > best.len() {
                best = ranked;
                used = Some(n as i64);
            }
            if ok {
                break;
            }
        }
        let now = now_ms();
        let found = matching::sufficient(&best);
        let status = if found { "found" } else { "not_found" };
        let error: Option<String> = (!found).then(|| match (&refused, best.is_empty(), made) {
            (Some(why), true, 0) => format!("Skipped: no search slskd accepted could be made of this title ({why})."),
            (None, true, 0) => "Nothing could be searched for in this title.".to_string(),
            (_, true, _) => "No one answered with this track.".to_string(),
            _ => "Every file found failed the rules.".to_string(),
        });
        app.db.write().execute(
            "UPDATE playlist_items SET status = ?, variation = ?, responses = ?, candidates = ?, chosen = ?, attempts = 0, error = ?, updated_at = ? WHERE id = ?",
            params![status, used, responses, candidates_json(&best), if found { Some(0i64) } else { None }, error, now, item.id],
        ).map_err(db_err)?;
        pl_log("info", "searched for a track", obj! { "run" => self.run_id, "item" => item.id, "found" => found, "variation" => used, "candidates" => best.len() as i64 });
        app.playlist.emit_item(app, item.id);
        app.playlist.emit_run(app, self.run_id, true);
        Ok(())
    }

    /// Queues downloads for tracks that were found, up to the limit.
    fn start_downloads(&self, app: &Arc<App>, rules: &PlaylistRules) {
        let c = app.db.read();
        let mut active = c.query_row("SELECT COUNT(*) FROM playlist_items WHERE run_id = ? AND status IN ('downloading', 'verifying')", params![self.run_id], |r| r.i64(0)).ok().flatten().unwrap_or(0);
        let found = load_items(&c, self.run_id, "status = 'found'").unwrap_or_default();
        drop(c);
        for item in found {
            if active >= rules.max_downloads || self.stopping() {
                break;
            }
            let accepted = item.accepted();
            let pick = item.chosen.unwrap_or(0).max(0) as usize;
            if item.attempts >= MAX_ATTEMPTS || pick >= accepted.len() {
                let why = if accepted.is_empty() {
                    "No file passed the rules.".to_string()
                } else {
                    format!("None of the {} files could be downloaded.", accepted.len().min(MAX_ATTEMPTS as usize))
                };
                let _ = app.db.write().execute("UPDATE playlist_items SET status = 'failed', error = ?, updated_at = ? WHERE id = ?", params![why, now_ms(), item.id]);
                app.playlist.emit_item(app, item.id);
                continue;
            }
            let c = accepted[pick].clone();
            match app.slskd.download(app, &c.username, &[(c.filename.clone(), c.size)]) {
                Ok(v) if v.get("queued").and_then(Value::as_i64).unwrap_or(0) > 0 => {
                    // The transfer slskd made for it: the newest of that file from that person.
                    let transfer = app.slskd.user_transfers(app, &c.username).ok().and_then(|ts| ts.into_iter().rev().find(|t| t.filename == c.filename && t.active()));
                    let now = now_ms();
                    let _ = app.db.write().execute(
                        "UPDATE playlist_items SET status = 'downloading', transfer_id = ?, transfer_user = ?, transfer_file = ?, transfer_size = ?, bytes_seen = 0, progress_at = ?, attempts = attempts + 1, error = NULL, updated_at = ? WHERE id = ?",
                        params![transfer.as_ref().map(|t| t.id.clone()), c.username, c.filename, c.size, now, now, item.id],
                    );
                    pl_log("info", "download queued", obj! { "run" => self.run_id, "item" => item.id, "attempt" => item.attempts + 1 });
                    active += 1;
                    app.playlist.emit_item(app, item.id);
                }
                Ok(_) => {
                    self.fallback(app, &item, "slskd could not ask for it", true);
                }
                Err(e) => {
                    if e.code == "slskd_unreachable" || e.code == "slskd_offline" {
                        break;
                    }
                    self.fallback(app, &item, &e.message, true);
                }
            }
        }
    }

    /// The pick did not work out: the next candidate is tried (up to
    /// `MAX_ATTEMPTS` in all), or the track is given up on.
    fn fallback(&self, app: &Arc<App>, item: &Item, why: &str, count: bool) {
        let Some(fresh) = load_item(&app.db.read(), item.id).ok().flatten() else { return };
        // The person chose another file (or the run moved on) since `item`
        // was read: this verdict is about a pick that is no longer there.
        if !still_the_pick(item, &fresh) {
            return;
        }
        let accepted = fresh.accepted().len() as i64;
        let next = fresh.chosen.unwrap_or(0) + 1;
        let attempts = fresh.attempts + if count { 1 } else { 0 };
        let pick = fresh.chosen.unwrap_or(0).max(0) as usize;
        let name = fresh.candidates.get(pick).map(|c| format!("{} from {}", c.name, c.username)).unwrap_or_else(|| "the pick".into());
        let now = now_ms();
        let again = next < accepted && attempts < MAX_ATTEMPTS;
        let note = match (&fresh.note, again) {
            (Some(n), true) => format!("{n} {name} did not work out ({why}); trying the next."),
            (None, true) => format!("{name} did not work out ({why}); trying the next."),
            (Some(n), false) => format!("{n} {name} did not work out ({why})."),
            (None, false) => format!("{name} did not work out ({why})."),
        };
        let status = if again { "found" } else { "failed" };
        let error: Option<String> = (!again).then(|| format!("Gave up after {} {}: {why}", attempts.max(1), if attempts.max(1) == 1 { "try" } else { "tries" }));
        let _ = app.db.write().execute(
            "UPDATE playlist_items SET status = ?, chosen = ?, attempts = ?, note = ?, error = ?, transfer_id = NULL, transfer_user = NULL, transfer_file = NULL, transfer_size = NULL, bytes_seen = NULL, progress_at = NULL, updated_at = ? WHERE id = ?",
            params![status, if again { next } else { fresh.chosen.unwrap_or(0) }, attempts, note, error, now, item.id],
        );
        pl_log("info", "pick did not work out", obj! { "run" => self.run_id, "item" => item.id, "again" => again });
        app.playlist.emit_item(app, item.id);
        app.playlist.emit_run(app, self.run_id, true);
    }

    /// Follows the downloads on their way and the files that arrived.
    fn tick_downloads(&self, app: &Arc<App>, rules: &PlaylistRules) {
        let items = load_items(&app.db.read(), self.run_id, "status IN ('downloading', 'verifying')").unwrap_or_default();
        for item in items {
            if self.stopping() {
                return;
            }
            let result = if item.status == "downloading" { self.tick_transfer(app, &item) } else { self.tick_arrival(app, &item, rules) };
            if let Tick::Done = result {
                app.playlist.emit_item(app, item.id);
                app.playlist.emit_run(app, self.run_id, false);
            }
        }
    }

    fn tick_transfer(&self, app: &Arc<App>, item: &Item) -> Tick {
        let (Some(user), Some(file)) = (item.transfer_user.clone(), item.transfer_file.clone()) else {
            self.fallback(app, item, "slskd gave it no transfer", false);
            return Tick::Done;
        };
        // The transfer, by id or (when slskd gave none back) by file.
        let found = match &item.transfer_id {
            Some(id) => app.slskd.transfer(app, &user, id),
            None => app.slskd.user_transfers(app, &user).map(|ts| ts.into_iter().rev().find(|t| t.filename == file)),
        };
        let t = match found {
            Ok(Some(t)) => t,
            Ok(None) => {
                self.fallback(app, item, "slskd no longer lists the download", false);
                return Tick::Done;
            }
            Err(_) => return Tick::Wait,
        };
        let now = now_ms();
        if item.transfer_id.is_none() {
            let _ = app.db.write().execute("UPDATE playlist_items SET transfer_id = ? WHERE id = ?", params![t.id.clone(), item.id]);
        }
        match t.state {
            "done" => {
                // The size slskd reports at the end is the file's (what the
                // other person's client announced may have been off).
                let size = if t.size > 0 { t.size } else { item.transfer_size.unwrap_or(0) };
                let _ = app.db.write().execute(
                    "UPDATE playlist_items SET status = 'verifying', transfer_size = ?, bytes_seen = ?, progress_at = ?, updated_at = ? WHERE id = ? AND status = 'downloading' AND transfer_file IS ?",
                    params![size, size, now, now, item.id, item.transfer_file.clone()],
                );
                Tick::Done
            }
            "failed" | "cancelled" => {
                let why = t.detail.clone().unwrap_or_else(|| if t.state == "cancelled" { "the download was cancelled".into() } else { "the download failed".into() });
                self.fallback(app, item, &why, false);
                Tick::Done
            }
            _ => {
                let moved = t.bytes > item.bytes_seen.unwrap_or(0);
                if moved {
                    let _ = app.db.write().execute(
                        "UPDATE playlist_items SET bytes_seen = ?, progress_at = ?, updated_at = ? WHERE id = ? AND status = 'downloading' AND transfer_file IS ?",
                        params![t.bytes, now, now, item.id, item.transfer_file.clone()],
                    );
                    return Tick::Done;
                }
                let since = item.progress_at.map(|p| (now - p).max(0)).unwrap_or(0) as u128;
                let limit = if t.state == "queued" { QUEUE_WAIT } else { STALL };
                if since > limit.as_millis() {
                    let _ = app.slskd.cancel_transfer(app, &user, &t.id, false);
                    let why = if t.state == "queued" { format!("still in {user}’s queue after an hour") } else { "no progress for 15 minutes".to_string() };
                    self.fallback(app, item, &why, false);
                    return Tick::Done;
                }
                Tick::Wait
            }
        }
    }

    fn tick_arrival(&self, app: &Arc<App>, item: &Item, rules: &PlaylistRules) -> Tick {
        let (Some(file), Some(size)) = (item.transfer_file.clone(), item.transfer_size) else {
            self.fallback(app, item, "the download left no file to look for", false);
            return Tick::Done;
        };
        let now = now_ms();
        let local = match crate::slskd::local_file(&app.db.read(), &file, size) {
            Ok(l) => l,
            Err(_) => return Tick::Wait,
        };
        let Some(local) = local else {
            let waited = item.progress_at.map(|p| (now - p).max(0)).unwrap_or(0) as u128;
            if waited > ARRIVAL_WAIT.as_millis() {
                self.fallback(app, item, "the finished file did not turn up in the download folder SONORA watches", false);
                return Tick::Done;
            }
            // SONORA's watcher looks every 20 seconds; asked to look now.
            app.sources.nudge();
            return Tick::Wait;
        };
        match local.state.as_str() {
            "review" => {
                let _ = app.db.write().execute("UPDATE playlist_items SET status = 'done', note = ?, updated_at = ? WHERE id = ?", params!["Waiting for your review under Downloads.", now, item.id]);
                Tick::Done
            }
            "declined" => {
                let _ = app.db.write().execute("UPDATE playlist_items SET status = 'done', note = 'You kept it out of the library.', updated_at = ? WHERE id = ?", params![now, item.id]);
                Tick::Done
            }
            "failed" => {
                self.fallback(app, item, &local.error.unwrap_or_else(|| "SONORA could not read it as audio".into()), false);
                Tick::Done
            }
            _ => {
                let waited = item.progress_at.map(|p| (now - p).max(0)).unwrap_or(0) as u128;
                let gone = |why: &str| {
                    let _ = app.db.write().execute("UPDATE playlist_items SET status = 'done', note = ?, updated_at = ? WHERE id = ? AND status = 'verifying'", params![why, now, item.id]);
                    pl_log("info", "arrived track not checked", obj! { "run" => self.run_id, "item" => item.id });
                    Tick::Done
                };
                let Some(track_id) = local.track_id else {
                    if local.state == "imported" {
                        return gone("It arrived; you removed it from the library since.");
                    }
                    if waited > CHECK_WAIT.as_millis() {
                        return gone("It arrived, but SONORA could not add it to the library to check it.");
                    }
                    return Tick::Wait;
                };
                let track = match library::get_track(&app.db.read(), track_id) {
                    Ok(Some(t)) => t,
                    Ok(None) => return gone("It arrived; you removed it from the library since."),
                    Err(_) => return Tick::Wait,
                };
                match track.status.as_str() {
                    "done" => {
                        // What it is made of, not what its name says: a WAV
                        // named .flac is fine, an MP3 named .flac is not.
                        let flag = |f: &str| track.flags.iter().any(|x| x == f);
                        let problem = if flag("source_concern") {
                            Some("SONORA found signs it was made from a lossy source")
                        } else if flag("integrity") {
                            Some("SONORA found damage or truncation in it")
                        } else if track.lossless == Some(false) && !rules.lossy {
                            Some("it turned out to be lossy")
                        } else if track.lossless == Some(false) && track.bitrate_kbps.is_some_and(|b| (b.round() as i64) < rules.min_bitrate) {
                            Some("its bit rate is below the minimum")
                        } else {
                            None
                        };
                        match problem {
                            Some(p) => {
                                let keep = format!("{p} (the file stays in the library as {}).", track.display_title());
                                let _ = app.db.write().execute("UPDATE playlist_items SET track_id = ?, updated_at = ? WHERE id = ?", params![track_id, now, item.id]);
                                self.fallback(app, item, &keep, false);
                            }
                            None => {
                                let _ = app.db.write().execute("UPDATE playlist_items SET status = 'done', track_id = ?, error = NULL, updated_at = ? WHERE id = ?", params![track_id, now, item.id]);
                                pl_log("info", "track verified", obj! { "run" => self.run_id, "item" => item.id, "track" => track_id });
                            }
                        }
                        Tick::Done
                    }
                    "failed" => {
                        self.fallback(app, item, &format!("it could not be analysed ({})", track.error_message.clone().unwrap_or_default()), false);
                        Tick::Done
                    }
                    _ if waited > CHECK_WAIT.as_millis() => {
                        let _ = app.db.write().execute("UPDATE playlist_items SET track_id = ? WHERE id = ?", params![track_id, item.id]);
                        gone("It is in the library, but its analysis has not run, so it was not checked. Analyze it from the library.")
                    }
                    _ => Tick::Wait,
                }
            }
        }
    }
}

#[cfg(test)]
mod playlist_tests {
    use super::*;

    #[test]
    fn a_verdict_on_an_earlier_pick_does_not_overrule_the_persons_choice() {
        // B15: the run looked at a download; meanwhile the person chose
        // another file, so the item is 'found' with a new pick.
        let seen = Item { status: "downloading".into(), chosen: Some(0), transfer_file: Some("@@a\\x.flac".into()), ..Item::default() };
        let chose = Item { status: "found".into(), chosen: Some(2), transfer_file: None, ..Item::default() };
        assert!(!still_the_pick(&seen, &chose));
        assert!(still_the_pick(&seen, &seen.clone()));
        let other_file = Item { transfer_file: Some("@@b\\y.flac".into()), ..seen.clone() };
        assert!(!still_the_pick(&seen, &other_file));
    }
}
