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

pub const SCHEMA_VERSION: i64 = 5;

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

/// Playlists: a tree of folders, playlists (tracks in the order the user
/// chose) and smart playlists (a saved filter), see `playlists.rs`.
const MIGRATION_3: &str = r#"
CREATE TABLE playlists (
    id INTEGER PRIMARY KEY,
    parent_id INTEGER REFERENCES playlists(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('folder', 'playlist', 'smart')),
    name TEXT NOT NULL,
    position INTEGER NOT NULL DEFAULT 0,
    rules TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX idx_playlists_parent ON playlists(parent_id, position);

CREATE TABLE playlist_tracks (
    playlist_id INTEGER NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
    track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    added_at INTEGER NOT NULL,
    PRIMARY KEY (playlist_id, track_id)
);
CREATE INDEX idx_playlist_tracks_order ON playlist_tracks(playlist_id, position);
CREATE INDEX idx_playlist_tracks_track ON playlist_tracks(track_id);
"#;

/// Files removed from the library: rescans of their folders leave them out
/// until the user imports the file again or brings them back.
const MIGRATION_4: &str = r#"
CREATE TABLE removed_files (
    path TEXT PRIMARY KEY,
    removed_at INTEGER NOT NULL
);
"#;

/// Files that arrived through a download client's folder (the person's own
/// Soulseek client): where each came from, and what happened to it. Nothing
/// about the people who shared them is kept.
const MIGRATION_5: &str = r#"
CREATE TABLE source_files (
    path TEXT PRIMARY KEY,
    source TEXT NOT NULL,
    size INTEGER NOT NULL,
    mtime INTEGER NOT NULL,
    state TEXT NOT NULL,
    track_id INTEGER REFERENCES tracks(id) ON DELETE SET NULL,
    found_at INTEGER NOT NULL,
    decided_at INTEGER,
    error TEXT
);
CREATE INDEX source_files_track ON source_files(track_id);
CREATE INDEX source_files_state ON source_files(state);
CREATE INDEX source_files_size ON source_files(size);
CREATE INDEX idx_tracks_size ON tracks(size);
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

    /// Writes a consistent copy of the database to `dest`. A file already
    /// there is replaced only with `replace` (the person confirmed it in a
    /// save dialog), and never when it is the library itself.
    pub fn backup_to(&self, dest: &Path, replace: bool) -> Result<(), StoreError> {
        if std::fs::symlink_metadata(dest).is_ok() {
            if !replace {
                return Err(StoreError::Other(format!("{} already exists", dest.display())));
            }
            if !dest.is_file() {
                return Err(StoreError::Other(format!("{} is not a file", dest.display())));
            }
            let target = dest.canonicalize().map_err(|e| StoreError::Io(e.to_string()))?;
            let live = ["", "-wal", "-shm", "-journal"].iter().any(|suffix| {
                let mut p = self.path.clone().into_os_string();
                p.push(suffix);
                PathBuf::from(p).canonicalize().is_ok_and(|p| p == target)
            });
            if live {
                return Err(StoreError::Other("that is SONORA's library itself: choose another name for the backup".into()));
            }
        }
        // Into a new file next to it first: a failed backup leaves an earlier
        // one there as it was.
        let name = dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "backup".into());
        let tmp = (1..1000)
            .map(|n| dest.with_file_name(format!(".{name}.{n}.sonora-partial")))
            .find(|p| std::fs::symlink_metadata(p).is_err())
            .ok_or_else(|| StoreError::Other(format!("no room for a temporary file next to {}", dest.display())))?;
        let written = {
            let w = self.write();
            match w.execute("VACUUM INTO ?", params![tmp.to_string_lossy().to_string()]) {
                Ok(_) => Ok(()),
                Err(_) => {
                    // Older SQLite without VACUUM INTO: checkpoint and copy.
                    w.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)").map_err(StoreError::from).and_then(|_| {
                        std::fs::copy(&self.path, &tmp).map(|_| ()).map_err(|e| StoreError::Io(e.to_string()))
                    })
                }
            }
        };
        // Replaces an existing backup on every system (MoveFileEx with
        // REPLACE_EXISTING on Windows).
        let result = written.and_then(|_| std::fs::rename(&tmp, dest).map_err(|e| StoreError::Io(e.to_string())));
        if result.is_err() {
            // Only the temporary file this call made.
            let _ = std::fs::remove_file(&tmp);
        }
        result
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
    if version < 3 {
        c.transaction(|tx| {
            tx.execute_batch(MIGRATION_3)?;
            tx.execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('schema_version', ?)", params!["3"])?;
            Ok::<(), DbError>(())
        })?;
    }
    if version < 4 {
        c.transaction(|tx| {
            tx.execute_batch(MIGRATION_4)?;
            tx.execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('schema_version', ?)", params!["4"])?;
            Ok::<(), DbError>(())
        })?;
    }
    if version < 5 {
        c.transaction(|tx| {
            tx.execute_batch(MIGRATION_5)?;
            tx.execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('schema_version', ?)", params!["5"])?;
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
        db.backup_to(&backup, false).unwrap();
        assert!(backup.exists());
        // An existing file is replaced only when the person said so...
        assert!(db.backup_to(&backup, false).is_err());
        db.write().execute("INSERT INTO settings (key, value) VALUES ('b', '2')", params![]).unwrap();
        db.backup_to(&backup, true).unwrap();
        let copy = Db::open(&backup).unwrap();
        let v = copy.read().query_row("SELECT value FROM settings WHERE key='b'", params![], |r| r.text(0)).unwrap();
        assert_eq!(v.as_deref(), Some("2"));
        drop(copy);
        // ...and never when it is the library itself.
        assert!(db.backup_to(&p, true).is_err());
        assert!(db.read().query_row("SELECT value FROM settings WHERE key='a'", params![], |r| r.text(0)).unwrap().is_some());
        // No temporary files are left next to the backup.
        let names: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert!(names.iter().all(|n| !n.ends_with(".sonora-partial")), "{names:?}");
    }
}
