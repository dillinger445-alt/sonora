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
    /// slskd set up by SONORA runs in the background from login on (true),
    /// or only while SONORA is open (false).
    pub slskd_background: bool,
    /// The person stopped SONORA's slskd: it stays stopped (at login too)
    /// until they start it.
    pub slskd_stopped: bool,
    /// How far a Soulseek preview reads ahead of what plays (or the player
    /// has read, if that is further), in seconds.
    pub preview_ahead_s: i64,
    /// Most disk space previews may use at a time, in megabytes.
    pub preview_cache_mb: i64,
    /// How a YouTube playlist's tracks are picked from Soulseek results.
    pub playlist_rules: PlaylistRules,
}

/// Formats a playlist download may accept, by extension: the lossless ones
/// the person orders, and the lossy ones (only with `lossy` on).
pub const LOSSLESS_FORMATS: &[&str] = &["flac", "aiff", "wav", "alac", "wv", "ape"];
pub const LOSSY_FORMATS: &[&str] = &["mp3", "m4a", "aac", "ogg", "opus"];

/// The rules a YouTube playlist download picks files by (Settings →
/// Playlist download rules; see `matching.rs`).
#[derive(Clone, Debug, PartialEq)]
pub struct PlaylistRules {
    /// Accepted lossless formats, best first (extensions from `LOSSLESS_FORMATS`).
    pub formats: Vec<String>,
    /// Lossy files too, after every lossless format, from `min_bitrate` kbps up.
    pub lossy: bool,
    pub min_bitrate: i64,
    /// "cd": prefer 16-bit/44.1 kHz; "highest": prefer the highest resolution; "any".
    pub quality: String,
    /// Reject above these (None: no limit).
    pub max_bit_depth: Option<i64>,
    pub max_sample_rate: Option<i64>,
    /// Reject files far smaller than a lossless file of that length should be.
    pub size_check: bool,
    /// Reject files larger than this (None: no limit).
    pub max_size_mb: Option<i64>,
    /// Only people with a free upload slot.
    pub free_slot: bool,
    /// Otherwise, a queue of at most this many downloads.
    pub max_queue: i64,
    /// Reported upload speed, in kB/s (0: any).
    pub min_speed_kbps: i64,
    /// Share of the title's words that must be in the file's name (0–1).
    pub min_overlap: f64,
    /// The artist must be in the file or folder name when it is known.
    pub require_artist: bool,
    /// Reject names with remix, edit, live, radio, instrumental… unless the title has them.
    pub reject_variants: bool,
    /// "automatic": download the best candidate as soon as a track is found; "review": show the picks first.
    pub mode: String,
    pub max_downloads: i64,
    /// Look again later for tracks that were not found.
    pub retry_not_found: bool,
    /// How long each search may run (15–25 s).
    pub search_seconds: i64,
}

impl Default for PlaylistRules {
    fn default() -> Self {
        PlaylistRules {
            formats: vec!["flac".into(), "aiff".into(), "wav".into(), "alac".into()],
            lossy: false,
            min_bitrate: 320,
            quality: "cd".into(),
            max_bit_depth: None,
            max_sample_rate: None,
            size_check: true,
            max_size_mb: None,
            free_slot: false,
            max_queue: 10,
            min_speed_kbps: 0,
            min_overlap: 0.6,
            require_artist: true,
            reject_variants: true,
            mode: "automatic".into(),
            max_downloads: 3,
            retry_not_found: true,
            search_seconds: 20,
        }
    }
}

impl PlaylistRules {
    /// The rules as saved, keeping every rule this version takes: a rule it
    /// does not know or no longer accepts (saved by another version) is
    /// left out, not all of them (B88).
    pub fn from_saved(v: &Value) -> PlaylistRules {
        let Some(o) = v.as_object() else { return PlaylistRules::default() };
        let mut good = Value::Object(Default::default());
        for (k, x) in o.iter() {
            let mut next = good.clone();
            next.set(k, x.clone());
            if PlaylistRules::from_json(&next).is_ok() {
                good = next;
            }
        }
        PlaylistRules::from_json(&good).unwrap_or_default()
    }

    /// Reads the rules from JSON; every key is optional and defaults.
    pub fn from_json(v: &Value) -> Result<PlaylistRules, String> {
        let mut r = PlaylistRules::default();
        let o = v.as_object().ok_or("playlistRules must be an object")?;
        let bad = |k: &str, what: &str| format!("playlistRules.{k} must be {what}");
        for (k, v) in o.iter() {
            match k {
                "formats" => {
                    let a = v.as_array().ok_or_else(|| bad(k, "a list of formats"))?;
                    let mut f = Vec::new();
                    for x in a {
                        let e = x.as_str().map(str::to_ascii_lowercase).ok_or_else(|| bad(k, "a list of formats"))?;
                        if !LOSSLESS_FORMATS.contains(&e.as_str()) {
                            return Err(format!("playlistRules.formats: {e} is not a lossless format SONORA knows (flac, aiff, wav, alac, wv, ape)"));
                        }
                        if !f.contains(&e) {
                            f.push(e);
                        }
                    }
                    r.formats = f;
                }
                "lossy" => r.lossy = v.as_bool().ok_or_else(|| bad(k, "true or false"))?,
                "minBitrate" => r.min_bitrate = v.as_i64().filter(|n| (64..=320).contains(n)).ok_or_else(|| bad(k, "between 64 and 320"))?,
                "quality" => {
                    let q = v.as_str().ok_or_else(|| bad(k, "cd, highest or any"))?;
                    if !["cd", "highest", "any"].contains(&q) {
                        return Err(bad(k, "cd, highest or any"));
                    }
                    r.quality = q.into();
                }
                "maxBitDepth" => r.max_bit_depth = if v.is_null() { None } else { Some(v.as_i64().filter(|n| [16, 24, 32].contains(n)).ok_or_else(|| bad(k, "16, 24, 32 or null"))?) },
                "maxSampleRate" => r.max_sample_rate = if v.is_null() { None } else { Some(v.as_i64().filter(|n| (44_100..=384_000).contains(n)).ok_or_else(|| bad(k, "a sample rate in Hz or null"))?) },
                "sizeCheck" => r.size_check = v.as_bool().ok_or_else(|| bad(k, "true or false"))?,
                "maxSizeMb" => r.max_size_mb = if v.is_null() { None } else { Some(v.as_i64().filter(|n| (1..=100_000).contains(n)).ok_or_else(|| bad(k, "megabytes or null"))?) },
                "freeSlot" => r.free_slot = v.as_bool().ok_or_else(|| bad(k, "true or false"))?,
                "maxQueue" => r.max_queue = v.as_i64().filter(|n| (0..=10_000).contains(n)).ok_or_else(|| bad(k, "a number of downloads"))?,
                "minSpeedKbps" => r.min_speed_kbps = v.as_i64().filter(|n| (0..=1_000_000).contains(n)).ok_or_else(|| bad(k, "kB/s"))?,
                "minOverlap" => r.min_overlap = v.as_f64().filter(|x| (0.0..=1.0).contains(x)).ok_or_else(|| bad(k, "between 0 and 1"))?,
                "requireArtist" => r.require_artist = v.as_bool().ok_or_else(|| bad(k, "true or false"))?,
                "rejectVariants" => r.reject_variants = v.as_bool().ok_or_else(|| bad(k, "true or false"))?,
                "mode" => {
                    let m = v.as_str().ok_or_else(|| bad(k, "automatic or review"))?;
                    if !["automatic", "review"].contains(&m) {
                        return Err(bad(k, "automatic or review"));
                    }
                    r.mode = m.into();
                }
                "maxDownloads" => r.max_downloads = v.as_i64().filter(|n| (1..=20).contains(n)).ok_or_else(|| bad(k, "between 1 and 20"))?,
                "retryNotFound" => r.retry_not_found = v.as_bool().ok_or_else(|| bad(k, "true or false"))?,
                "searchSeconds" => r.search_seconds = v.as_i64().filter(|n| (15..=25).contains(n)).ok_or_else(|| bad(k, "between 15 and 25 seconds"))?,
                other => return Err(format!("unknown playlist rule '{other}'")),
            }
        }
        if r.formats.is_empty() && !r.lossy {
            return Err("playlistRules: choose at least one format".into());
        }
        Ok(r)
    }
}

impl ToJson for PlaylistRules {
    fn to_json(&self) -> Value {
        obj! {
            "formats" => Value::Array(self.formats.iter().map(|f| Value::String(f.clone())).collect()),
            "lossy" => self.lossy,
            "minBitrate" => self.min_bitrate,
            "quality" => self.quality,
            "maxBitDepth" => self.max_bit_depth,
            "maxSampleRate" => self.max_sample_rate,
            "sizeCheck" => self.size_check,
            "maxSizeMb" => self.max_size_mb,
            "freeSlot" => self.free_slot,
            "maxQueue" => self.max_queue,
            "minSpeedKbps" => self.min_speed_kbps,
            "minOverlap" => sonora_json::num(self.min_overlap, 2),
            "requireArtist" => self.require_artist,
            "rejectVariants" => self.reject_variants,
            "mode" => self.mode,
            "maxDownloads" => self.max_downloads,
            "retryNotFound" => self.retry_not_found,
            "searchSeconds" => self.search_seconds,
        }
    }
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
            slskd_background: true,
            slskd_stopped: false,
            preview_ahead_s: 60,
            preview_cache_mb: 1024,
            playlist_rules: PlaylistRules::default(),
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
                if k == "playlistRules" {
                    s.playlist_rules = PlaylistRules::from_saved(&val);
                    continue;
                }
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
            "slskdBackground" => self.slskd_background = v.as_bool().ok_or("slskdBackground must be true or false")?,
            "slskdStopped" => self.slskd_stopped = v.as_bool().ok_or("slskdStopped must be true or false")?,
            "previewAhead" => {
                let n = v.as_i64().ok_or("previewAhead must be a number of seconds")?;
                if !(20..=600).contains(&n) {
                    return Err("previewAhead must be between 20 and 600 seconds".into());
                }
                self.preview_ahead_s = n;
            }
            "previewCacheMb" => {
                let n = v.as_i64().ok_or("previewCacheMb must be a number of megabytes")?;
                if !(128..=16384).contains(&n) {
                    return Err("previewCacheMb must be between 128 and 16384".into());
                }
                self.preview_cache_mb = n;
            }
            "playlistRules" => self.playlist_rules = PlaylistRules::from_json(v)?,
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
            "slskdBackground" => self.slskd_background,
            "slskdStopped" => self.slskd_stopped,
            "previewAhead" => self.preview_ahead_s,
            "previewCacheMb" => self.preview_cache_mb,
            "playlistRules" => self.playlist_rules.to_json(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_rules_keep_what_this_version_takes() {
        // B88: one rule from another version used to reset them all.
        let saved = sonora_json::parse(r#"{"formats":["flac"],"maxQueue":5,"futureRule":1,"maxDownloads":99,"mode":"review"}"#).unwrap();
        let r = PlaylistRules::from_saved(&saved);
        assert_eq!(r.formats, vec!["flac".to_string()]);
        assert_eq!((r.max_queue, r.mode.as_str()), (5, "review"));
        assert_eq!(r.max_downloads, PlaylistRules::default().max_downloads, "the value out of range is left out");
        // Changes through the API stay strict.
        assert!(PlaylistRules::from_json(&saved).is_err());
    }

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
    fn playlist_rules_are_checked_and_kept() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL)").unwrap();
        let mut s = Settings::default();
        let r = &s.playlist_rules;
        assert_eq!(r.formats, ["flac", "aiff", "wav", "alac"]);
        assert!(!r.lossy && r.size_check && r.require_artist && r.reject_variants && r.retry_not_found);
        assert_eq!((r.quality.as_str(), r.mode.as_str(), r.min_bitrate, r.max_queue, r.max_downloads, r.search_seconds), ("cd", "automatic", 320, 10, 3, 20));
        assert!(s.apply("playlistRules", &obj! { "formats" => Value::Array(vec![Value::String("mp3".into())]) }).is_err(), "lossy formats are a switch, not a format");
        assert!(s.apply("playlistRules", &obj! { "formats" => Value::Array(vec![]) }).is_err(), "at least one format");
        assert!(s.apply("playlistRules", &obj! { "searchSeconds" => 40 }).is_err());
        assert!(s.apply("playlistRules", &obj! { "quality" => "best" }).is_err());
        assert!(s.apply("playlistRules", &obj! { "minOverlap" => 1.5 }).is_err());
        assert!(s.apply("playlistRules", &obj! { "bogus" => true }).is_err());
        s.apply("playlistRules", &obj! { "formats" => Value::Array(vec![Value::String("FLAC".into()), Value::String("wav".into()), Value::String("flac".into())]), "lossy" => true, "minBitrate" => 256, "quality" => "highest", "maxBitDepth" => 24, "maxSampleRate" => 96000, "mode" => "review", "maxDownloads" => 2, "searchSeconds" => 25, "minOverlap" => 0.5 })
            .unwrap();
        s.save(&c).unwrap();
        let loaded = Settings::load(&c).playlist_rules;
        assert_eq!(loaded.formats, ["flac", "wav"], "lower case, no repeats");
        assert!(loaded.lossy);
        assert_eq!((loaded.min_bitrate, loaded.quality.as_str(), loaded.max_bit_depth, loaded.max_sample_rate, loaded.mode.as_str(), loaded.max_downloads, loaded.search_seconds), (256, "highest", Some(24), Some(96_000), "review", 2, 25));
        assert!((loaded.min_overlap - 0.5).abs() < 1e-9);
        // A rule left out keeps its default; null clears a limit.
        let s2 = PlaylistRules::from_json(&obj! { "maxBitDepth" => Value::Null }).unwrap();
        assert_eq!(s2.max_bit_depth, None);
        assert_eq!(s2.formats, ["flac", "aiff", "wav", "alac"]);
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
