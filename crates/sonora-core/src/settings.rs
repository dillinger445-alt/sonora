//! User settings, persisted as JSON values in the `settings` table.

use crate::db::sqlite::Connection;
use crate::params;
use sonora_json::{obj, ToJson, Value};

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// Parallel analysis workers.
    pub workers: usize,
    /// Score shown by default in the library: "overall" | "dj" | "listening".
    pub default_score: String,
    /// On launch: "off" | "changed" (rescan library folders for new/changed files).
    pub rescan_on_launch: String,
    /// Re-analyse files whose size or modification time changed.
    pub reanalyze_changed: bool,
    pub skip_hidden: bool,
    pub follow_symlinks: bool,
    /// Longest file analysed, in minutes.
    pub max_duration_min: f64,
    /// Store waveform/spectrogram data (can be cleared to save space).
    pub keep_visuals: bool,
    /// Interface theme: only "dark" is implemented.
    pub theme: String,
}

pub fn default_workers() -> usize {
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2);
    // Leave headroom for the interface and FFmpeg decoders.
    cores.saturating_sub(1).clamp(1, 6)
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            workers: default_workers(),
            default_score: "overall".into(),
            rescan_on_launch: "off".into(),
            reanalyze_changed: true,
            skip_hidden: true,
            follow_symlinks: false,
            max_duration_min: 240.0,
            keep_visuals: true,
            theme: "dark".into(),
        }
    }
}

pub const MAX_WORKERS: usize = 16;

impl Settings {
    pub fn load(c: &Connection) -> Settings {
        let mut s = Settings::default();
        let rows = c.query("SELECT key, value FROM settings", params![], |r| (r.text(0), r.text(1))).unwrap_or_default();
        for (k, v) in rows {
            if let Ok(val) = sonora_json::parse(&v) {
                let _ = s.apply(&k, &val);
            }
        }
        s
    }

    /// Applies one setting, validating the value.
    pub fn apply(&mut self, key: &str, v: &Value) -> Result<(), String> {
        match key {
            "workers" => {
                let n = v.as_i64().ok_or("workers must be a number")?;
                if !(1..=MAX_WORKERS as i64).contains(&n) {
                    return Err(format!("workers must be between 1 and {MAX_WORKERS}"));
                }
                self.workers = n as usize;
            }
            "defaultScore" => {
                let s = v.as_str().ok_or("defaultScore must be a string")?;
                if !["overall", "dj", "listening"].contains(&s) {
                    return Err("defaultScore must be overall, dj or listening".into());
                }
                self.default_score = s.into();
            }
            "rescanOnLaunch" => {
                let s = v.as_str().ok_or("rescanOnLaunch must be a string")?;
                if !["off", "changed"].contains(&s) {
                    return Err("rescanOnLaunch must be off or changed".into());
                }
                self.rescan_on_launch = s.into();
            }
            "reanalyzeChanged" => self.reanalyze_changed = v.as_bool().ok_or("reanalyzeChanged must be true or false")?,
            "skipHidden" => self.skip_hidden = v.as_bool().ok_or("skipHidden must be true or false")?,
            "followSymlinks" => self.follow_symlinks = v.as_bool().ok_or("followSymlinks must be true or false")?,
            "keepVisuals" => self.keep_visuals = v.as_bool().ok_or("keepVisuals must be true or false")?,
            "maxDurationMin" => {
                let m = v.as_f64().ok_or("maxDurationMin must be a number")?;
                if !(1.0..=24.0 * 60.0).contains(&m) {
                    return Err("maxDurationMin must be between 1 and 1440".into());
                }
                self.max_duration_min = m;
            }
            "theme" => {
                let s = v.as_str().ok_or("theme must be a string")?;
                if s != "dark" {
                    return Err("only the dark theme is available".into());
                }
                self.theme = s.into();
            }
            other => return Err(format!("unknown setting '{other}'")),
        }
        Ok(())
    }

    pub fn save(&self, c: &Connection) -> Result<(), crate::db::sqlite::DbError> {
        let json = self.to_json();
        if let Value::Object(map) = json {
            for (k, v) in map.iter() {
                c.execute("INSERT INTO settings (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value", params![k, v.to_string_compact()])?;
            }
        }
        Ok(())
    }
}

impl ToJson for Settings {
    fn to_json(&self) -> Value {
        obj! {
            "workers" => self.workers,
            "defaultScore" => self.default_score,
            "rescanOnLaunch" => self.rescan_on_launch,
            "reanalyzeChanged" => self.reanalyze_changed,
            "skipHidden" => self.skip_hidden,
            "followSymlinks" => self.follow_symlinks,
            "maxDurationMin" => self.max_duration_min,
            "keepVisuals" => self.keep_visuals,
            "theme" => self.theme,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_and_persist() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL)").unwrap();
        let mut s = Settings::default();
        assert!(s.apply("workers", &Value::Int(0)).is_err());
        assert!(s.apply("workers", &Value::Int(99)).is_err());
        assert!(s.apply("defaultScore", &Value::String("loudness".into())).is_err());
        assert!(s.apply("bogus", &Value::Bool(true)).is_err());
        s.apply("workers", &Value::Int(3)).unwrap();
        s.apply("defaultScore", &Value::String("dj".into())).unwrap();
        s.save(&c).unwrap();
        let loaded = Settings::load(&c);
        assert_eq!(loaded.workers, 3);
        assert_eq!(loaded.default_score, "dj");
    }
}
