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
                shared.index_cancel.store(false, Ordering::Relaxed);
                let result = match &job {
                    IndexJob::Paths(p) => library::import_paths(&shared.db, p, &opts, &shared.index_cancel),
                    IndexJob::Folders(ids) => library::rescan_folders(&shared.db, ids.as_deref(), &opts, &shared.index_cancel),
                    IndexJob::Force(p) => {
                        let mut o = opts;
                        o.force = true;
                        library::import_paths(&shared.db, p, &o, &shared.index_cancel)
                    }
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
        if with_tracks {
            if st.phase == Phase::Indexing || st.pending_index_jobs > 0 {
                return Ok(None);
            }
            let ids = self.shared.db.read().query("SELECT id FROM tracks WHERE folder_id = ?", params![id], |r| r.i64(0))?;
            let (free, busy) = split_busy(&st, &ids);
            if busy > 0 {
                return Ok(None);
            }
            // The folder goes, so there is nothing for its rescans to leave out.
            n = self.remove_locked(&mut st, &free, false)?;
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
        let (rows, busy) = {
            let mut st = lock(&self.shared.state);
            let (free, busy) = split_busy(&st, ids);
            self.dequeue_locked(&mut st, &free)?;
            (library::get_tracks(&self.shared.db.read(), &free)?, busy)
        };
        // Moving can take a while (a USB stick, a network share): without the
        // lock, so analysis and status calls carry on meanwhile.
        let mut out = Trashed { busy, ..Trashed::default() };
        let mut gone = Vec::new();
        for r in &rows {
            let p = Path::new(&r.path);
            if std::fs::symlink_metadata(p).is_err() {
                gone.push(r.id);
                continue;
            }
            match crate::trash::move_to_trash(p) {
                Ok(()) => {
                    out.trashed += 1;
                    gone.push(r.id);
                }
                Err(e) => out.failed.push((r.id, r.display_title(), e)),
            }
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
    /// analysed); returns how many were waiting in it.
    fn dequeue_locked(&self, st: &mut RunState, ids: &[i64]) -> Result<i64, StoreError> {
        let Some(scan_id) = st.scan_id else { return Ok(0) };
        if ids.is_empty() {
            return Ok(0);
        }
        let c = self.shared.db.write();
        let mut dequeued = 0i64;
        for chunk in ids.chunks(500) {
            let marks = vec!["?"; chunk.len()].join(",");
            let mut p = vec![SqlValue::Int(scan_id)];
            p.extend(chunk.iter().map(|&i| SqlValue::Int(i)));
            let sql = format!("DELETE FROM scan_items WHERE scan_id = ? AND outcome = 'queued' AND track_id IN ({marks})");
            dequeued += c.execute(&sql, &library::as_params(&p))? as i64;
        }
        if dequeued > 0 {
            c.execute("UPDATE scans SET total = MAX(0, total - ?) WHERE id = ?", params![dequeued, scan_id])?;
            st.total = (st.total - dequeued).max(0);
        }
        drop(c);
        if dequeued > 0 {
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
        self.shared.index_cancel.store(true, Ordering::Relaxed);
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
}

impl IndexJob {
    fn describe(&self) -> String {
        match self {
            IndexJob::Paths(p) | IndexJob::Force(p) if p.len() == 1 => format!("Indexing {}", p[0].display()),
            IndexJob::Paths(p) | IndexJob::Force(p) => format!("Indexing {} locations", p.len()),
            IndexJob::Folders(_) => "Checking library folders for changes".to_string(),
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
        for w in 0..want {
            let sh = Arc::clone(shared);
            st.workers_alive += 1;
            match std::thread::Builder::new().name(format!("sonora-worker-{w}")).spawn(move || worker(sh, w)) {
                Ok(h) => st.handles.push(h),
                Err(e) => {
                    st.workers_alive -= 1;
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
    let elapsed = st.started.elapsed().saturating_sub(st.paused_total);
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
    let workers = st.slots.len().max(1) as f64;
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
        "workers" => st.slots.len(),
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
