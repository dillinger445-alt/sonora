//! User settings, persisted as JSON values in the `settings` table.

use crate::db::sqlite::Connection;
use crate::dj::{DjConfig, FolderGenre};
use crate::params;
use sonora_engine::analysis::key::Notation;
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
    /// How keys are written: "camelot" | "openkey" | "musical" | "sharps" | "flats".
    pub key_notation: String,
    /// Tempos are shown within this range (at least an octave wide).
    pub bpm_range: (f64, f64),
    /// Genre from folder names when the tags have none: "off" | "top" | "parent".
    pub genre_from_folders: String,
    /// The desktop app asks GitHub for a newer release on launch and every
    /// few hours (the interface does the asking; the core only keeps the choice).
    pub check_updates: bool,
    /// The folder where the person's own Soulseek client saves finished
    /// downloads; SONORA adds what arrives there. None = not watched.
    pub soulseek_folder: Option<String>,
    /// Hold each arrival for the person to add or keep out, instead of
    /// adding it at once.
    pub soulseek_review: bool,
    /// Where slskd, the Soulseek client SONORA can search through, answers.
    pub slskd_url: String,
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
            key_notation: "camelot".into(),
            bpm_range: sonora_engine::analysis::tempo::DEFAULT_RANGE,
            genre_from_folders: "off".into(),
            check_updates: true,
            soulseek_folder: None,
            soulseek_review: false,
            slskd_url: crate::slskd::DEFAULT_URL.into(),
        }
    }
}

pub const MAX_WORKERS: usize = 16;

impl Settings {
    /// The settings that shape the tempo, key and genre shown.
    pub fn dj_config(&self) -> DjConfig {
        DjConfig { bpm_range: self.bpm_range, folder_genre: FolderGenre::from_id(&self.genre_from_folders).unwrap_or(FolderGenre::Off) }
    }

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
            "checkUpdates" => self.check_updates = v.as_bool().ok_or("checkUpdates must be true or false")?,
            "soulseekReview" => self.soulseek_review = v.as_bool().ok_or("soulseekReview must be true or false")?,
            "soulseekFolder" => {
                if v.is_null() {
                    self.soulseek_folder = None;
                } else {
                    let p = v.as_str().ok_or("soulseekFolder must be a folder path")?;
                    if !std::path::Path::new(p).is_absolute() {
                        return Err("soulseekFolder must be a full path".into());
                    }
                    self.soulseek_folder = Some(p.to_string());
                }
            }
            "slskdUrl" => {
                let u = v.as_str().ok_or("slskdUrl must be a web address")?;
                self.slskd_url = crate::slskd::normalize_url(u)?;
            }
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
            "keyNotation" => {
                let s = v.as_str().ok_or("keyNotation must be a string")?;
                if Notation::from_id(s).is_none() {
                    return Err("keyNotation must be camelot, openkey, musical, sharps or flats".into());
                }
                self.key_notation = s.into();
            }
            "bpmRange" => {
                let a = v.as_array().filter(|a| a.len() == 2).ok_or("bpmRange must be [lowest, highest]")?;
                let (lo, hi) = (a[0].as_f64().ok_or("bpmRange must hold numbers")?, a[1].as_f64().ok_or("bpmRange must hold numbers")?);
                if !(40.0..=150.0).contains(&lo) || hi > 300.0 {
                    return Err("bpmRange must lie between 40 and 300 BPM".into());
                }
                // Narrower than an octave, some tempos could not be shown at all.
                if hi < lo * 1.95 {
                    return Err("bpmRange must span at least an octave (the highest about twice the lowest)".into());
                }
                self.bpm_range = (lo, hi);
            }
            "genreFromFolders" => {
                let s = v.as_str().ok_or("genreFromFolders must be a string")?;
                if FolderGenre::from_id(s).is_none() {
                    return Err("genreFromFolders must be off, top or parent".into());
                }
                self.genre_from_folders = s.into();
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
            "keyNotation" => self.key_notation,
            "bpmRange" => Value::Array(vec![Value::from(self.bpm_range.0), Value::from(self.bpm_range.1)]),
            "genreFromFolders" => self.genre_from_folders,
            "checkUpdates" => self.check_updates,
            "soulseekFolder" => self.soulseek_folder.clone().map(Value::String).unwrap_or(Value::Null),
            "soulseekReview" => self.soulseek_review,
            "slskdUrl" => self.slskd_url,
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

    #[test]
    fn dj_settings() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL)").unwrap();
        let mut s = Settings::default();
        let range = |a: f64, b: f64| Value::Array(vec![Value::from(a), Value::from(b)]);
        assert!(s.apply("bpmRange", &range(90.0, 120.0)).is_err(), "narrower than an octave");
        assert!(s.apply("bpmRange", &range(10.0, 200.0)).is_err());
        assert!(s.apply("keyNotation", &Value::String("solfege".into())).is_err());
        assert!(s.apply("genreFromFolders", &Value::String("all".into())).is_err());
        s.apply("bpmRange", &range(88.0, 175.0)).unwrap();
        s.apply("keyNotation", &Value::String("openkey".into())).unwrap();
        s.apply("genreFromFolders", &Value::String("top".into())).unwrap();
        assert!(s.check_updates, "update checks are on unless turned off");
        assert!(s.apply("checkUpdates", &Value::String("yes".into())).is_err());
        s.apply("checkUpdates", &Value::Bool(false)).unwrap();
        s.save(&c).unwrap();
        let loaded = Settings::load(&c);
        assert!(!loaded.check_updates);
        assert_eq!(loaded.bpm_range, (88.0, 175.0));
        assert_eq!(loaded.key_notation, "openkey");
        assert_eq!(loaded.dj_config().folder_genre, FolderGenre::Top);
    }
}
