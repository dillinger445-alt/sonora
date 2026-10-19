//! Persistent storage (SQLite).
//!
//! Two connections in WAL mode: a *writer* used by the scan workers and by
//! mutating API calls, and a *reader* used by queries, so the interface stays
//! responsive while analyses are being written. Each connection sits behind
//! its own mutex. Every per-track write happens in one transaction, so a
//! cancelled or crashed scan never leaves half-written results.

pub mod sqlite;

use crate::params;
use sqlite::{Connection, DbError};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

pub const SCHEMA_VERSION: i64 = 2;

const MIGRATION_1: &str = r#"
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);

CREATE TABLE folders (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL UNIQUE,
    added_at INTEGER NOT NULL,
    last_scan_at INTEGER,
    available INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE tracks (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL UNIQUE,
    folder_id INTEGER REFERENCES folders(id) ON DELETE SET NULL,
    file_name TEXT NOT NULL,
    ext TEXT NOT NULL,
    size INTEGER NOT NULL,
    mtime INTEGER NOT NULL,
    quick_hash TEXT NOT NULL,
    file_hash TEXT,
    audio_hash TEXT,
    status TEXT NOT NULL,
    error_code TEXT,
    error_message TEXT,
    added_at INTEGER NOT NULL,
    analyzed_at INTEGER,
    missing_since INTEGER,
    title TEXT,
    artist TEXT,
    album TEXT,
    genre TEXT,
    bpm REAL,
    musical_key TEXT,
    container TEXT,
    codec TEXT,
    codec_name TEXT,
    format TEXT,
    lossless INTEGER,
    sample_rate INTEGER,
    bit_depth INTEGER,
    bitrate_kbps REAL,
    channels INTEGER,
    duration REAL,
    integrated_lufs REAL,
    true_peak REAL,
    cutoff_hz REAL,
    upper_hz REAL,
    overall REAL,
    dj REAL,
    listening REAL,
    confidence REAL,
    confidence_label TEXT,
    verdict TEXT,
    flags TEXT NOT NULL DEFAULT '',
    model_version TEXT,
    engine_version TEXT,
    sort_title TEXT NOT NULL DEFAULT ''
);
CREATE INDEX idx_tracks_folder ON tracks(folder_id);
CREATE INDEX idx_tracks_status ON tracks(status);
CREATE INDEX idx_tracks_quick ON tracks(quick_hash, size);
CREATE INDEX idx_tracks_file_hash ON tracks(file_hash);
CREATE INDEX idx_tracks_audio_hash ON tracks(audio_hash);
CREATE INDEX idx_tracks_overall ON tracks(overall);
CREATE INDEX idx_tracks_analyzed ON tracks(analyzed_at);
CREATE INDEX idx_tracks_format ON tracks(format);

CREATE TABLE analyses (
    track_id INTEGER PRIMARY KEY REFERENCES tracks(id) ON DELETE CASCADE,
    facts TEXT NOT NULL,
    measurements TEXT NOT NULL,
    assessment TEXT NOT NULL,
    visuals TEXT,
    fingerprint BLOB,
    created_at INTEGER NOT NULL
);

CREATE TABLE scans (
    id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL,
    state TEXT NOT NULL,
    started_at INTEGER NOT NULL,
    finished_at INTEGER,
    roots TEXT NOT NULL DEFAULT '[]',
    total INTEGER NOT NULL DEFAULT 0,
    completed INTEGER NOT NULL DEFAULT 0,
    failed INTEGER NOT NULL DEFAULT 0,
    skipped INTEGER NOT NULL DEFAULT 0,
    cancelled INTEGER NOT NULL DEFAULT 0,
    summary TEXT
);

CREATE TABLE scan_items (
    scan_id INTEGER NOT NULL REFERENCES scans(id) ON DELETE CASCADE,
    track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    seq INTEGER NOT NULL,
    outcome TEXT NOT NULL,
    finished_at INTEGER,
    PRIMARY KEY (scan_id, track_id)
);
CREATE INDEX idx_scan_items_queue ON scan_items(scan_id, outcome, seq);
CREATE INDEX idx_scan_items_track ON scan_items(track_id);

CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);

CREATE TABLE fp_matches (
    a INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    b INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    ber REAL NOT NULL,
    offset_secs REAL NOT NULL,
    matched_secs REAL NOT NULL,
    PRIMARY KEY (a, b)
);
"#;

/// DJ metadata: detected tempo and key, the user's edits, and the values in
/// use (see `dj.rs`).
const MIGRATION_2: &str = r#"
ALTER TABLE tracks ADD COLUMN dj_version INTEGER;
ALTER TABLE tracks ADD COLUMN bpm_detected REAL;
ALTER TABLE tracks ADD COLUMN bpm_confidence REAL;
ALTER TABLE tracks ADD COLUMN tempo_variable INTEGER;
ALTER TABLE tracks ADD COLUMN tempo_candidates TEXT;
ALTER TABLE tracks ADD COLUMN key_detected INTEGER;
ALTER TABLE tracks ADD COLUMN key_confidence REAL;
ALTER TABLE tracks ADD COLUMN key_second INTEGER;
ALTER TABLE tracks ADD COLUMN bpm_edit REAL;
ALTER TABLE tracks ADD COLUMN key_edit INTEGER;
ALTER TABLE tracks ADD COLUMN genre_edit TEXT;
ALTER TABLE tracks ADD COLUMN genre_edit_source TEXT;
ALTER TABLE tracks ADD COLUMN discogs_url TEXT;
ALTER TABLE tracks ADD COLUMN dj_bpm REAL;
ALTER TABLE tracks ADD COLUMN dj_bpm_source TEXT;
ALTER TABLE tracks ADD COLUMN dj_key INTEGER;
ALTER TABLE tracks ADD COLUMN dj_key_source TEXT;
ALTER TABLE tracks ADD COLUMN dj_key_order INTEGER;
ALTER TABLE tracks ADD COLUMN dj_genre TEXT;
ALTER TABLE tracks ADD COLUMN dj_genre_source TEXT;
CREATE INDEX idx_tracks_dj_bpm ON tracks(dj_bpm);
CREATE INDEX idx_tracks_dj_key ON tracks(dj_key);
CREATE INDEX idx_tracks_dj_genre ON tracks(dj_genre COLLATE NOCASE);
INSERT OR REPLACE INTO meta (key, value) VALUES ('dj_refresh', '1');
"#;

#[derive(Debug)]
pub enum StoreError {
    Db(DbError),
    Io(String),
    Other(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Db(e) => write!(f, "{e}"),
            StoreError::Io(e) => write!(f, "{e}"),
            StoreError::Other(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<DbError> for StoreError {
    fn from(e: DbError) -> Self {
        StoreError::Db(e)
    }
}

pub struct Db {
    path: PathBuf,
    writer: Mutex<Connection>,
    reader: Mutex<Connection>,
}

fn configure(c: &Connection) -> Result<(), DbError> {
    c.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA temp_store = MEMORY;")
}

fn lock(m: &Mutex<Connection>) -> MutexGuard<'_, Connection> {
    // A panic while holding the lock leaves SQLite itself consistent
    // (transactions roll back); keep going with the inner value.
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl Db {
    /// Opens (creating if needed) and migrates the database at `path`.
    pub fn open(path: &Path) -> Result<Db, StoreError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| StoreError::Io(format!("cannot create {}: {e}", dir.display())))?;
        }
        let writer = Connection::open(path)?;
        configure(&writer)?;
        migrate(&writer)?;
        let reader = Connection::open(path)?;
        configure(&reader)?;
        Ok(Db { path: path.to_path_buf(), writer: Mutex::new(writer), reader: Mutex::new(reader) })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn write(&self) -> MutexGuard<'_, Connection> {
        lock(&self.writer)
    }

    pub fn read(&self) -> MutexGuard<'_, Connection> {
        lock(&self.reader)
    }

    /// Size of the database files on disk (main file + WAL).
    pub fn size_bytes(&self) -> u64 {
        let wal = PathBuf::from(format!("{}-wal", self.path.display()));
        std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0) + std::fs::metadata(wal).map(|m| m.len()).unwrap_or(0)
    }

    /// Writes a consistent copy of the database to `dest`.
    pub fn backup_to(&self, dest: &Path) -> Result<(), StoreError> {
        if dest.exists() {
            return Err(StoreError::Other(format!("{} already exists", dest.display())));
        }
        let w = self.write();
        let target = dest.to_string_lossy().to_string();
        match w.execute("VACUUM INTO ?", params![target]) {
            Ok(_) => Ok(()),
            Err(_) => {
                // Older SQLite without VACUUM INTO: checkpoint and copy.
                w.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
                std::fs::copy(&self.path, dest).map(|_| ()).map_err(|e| StoreError::Io(e.to_string()))
            }
        }
    }
}

fn migrate(c: &Connection) -> Result<(), DbError> {
    c.execute_batch("CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)")?;
    let version: i64 = c
        .query_row("SELECT value FROM meta WHERE key = 'schema_version'", params![], |r| r.text(0))?
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if version > SCHEMA_VERSION {
        return Err(DbError { code: 1, message: format!("database schema {version} is newer than this version of SONORA supports ({SCHEMA_VERSION})") });
    }
    if version < 1 {
        c.transaction(|tx| {
            tx.execute_batch(MIGRATION_1)?;
            tx.execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('schema_version', ?)", params!["1"])?;
            Ok::<(), DbError>(())
        })?;
    }
    if version < 2 {
        c.transaction(|tx| {
            tx.execute_batch(MIGRATION_2)?;
            tx.execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('schema_version', ?)", params!["2"])?;
            Ok::<(), DbError>(())
        })?;
    }
    Ok(())
}

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_migrate_reopen() {
        let dir = std::env::temp_dir().join(format!("sonora-db-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let p = dir.join("x").join("sonora.db");
        {
            let db = Db::open(&p).unwrap();
            db.write().execute("INSERT INTO settings (key, value) VALUES ('a', '1')", params![]).unwrap();
        }
        let db = Db::open(&p).unwrap();
        let v = db.read().query_row("SELECT value FROM settings WHERE key='a'", params![], |r| r.text(0)).unwrap();
        assert_eq!(v.as_deref(), Some("1"));
        let mode = db.read().query_row("PRAGMA journal_mode", params![], |r| r.text(0)).unwrap();
        assert_eq!(mode.as_deref(), Some("wal"));
        let backup = dir.join("backup.db");
        db.backup_to(&backup).unwrap();
        assert!(backup.exists());
        assert!(db.backup_to(&backup).is_err());
    }
}
