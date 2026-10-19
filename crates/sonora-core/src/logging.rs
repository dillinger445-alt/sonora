//! Structured logging: JSON lines to `<data dir>/logs/sonora.log` (rotated
//! at 5 MB, one previous file kept) and to stderr.

use sonora_json::{obj, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

static LOG_FILE: Mutex<Option<PathBuf>> = Mutex::new(None);
const MAX_BYTES: u64 = 5 * 1024 * 1024;

pub fn init(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    *LOG_FILE.lock().unwrap_or_else(|p| p.into_inner()) = Some(dir.join("sonora.log"));
}

pub fn log(level: &str, target: &str, message: &str) {
    log_fields(level, target, message, Value::Null);
}

pub fn log_fields(level: &str, target: &str, message: &str, fields: Value) {
    let ts = crate::export::iso_utc(crate::db::now_ms());
    let line = obj! { "ts" => ts, "level" => level, "target" => target, "msg" => message, "fields" => fields }.to_string_compact();
    eprintln!("{line}");
    let guard = LOG_FILE.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(path) = guard.as_ref() {
        if std::fs::metadata(path).map(|m| m.len() > MAX_BYTES).unwrap_or(false) {
            let _ = std::fs::rename(path, path.with_extension("log.1"));
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{line}");
        }
    }
}
