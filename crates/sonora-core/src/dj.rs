//! DJ metadata: the tempo, key and genre shown, filtered, sorted and exported.
//!
//! A field can come from several places: the value detected by the analysis,
//! the file's own tags, the user's edit, and for genres also a Discogs lookup
//! or the name of the folder the file is in. The value in use is stored in
//! the `dj_*` columns, with where it came from, and is recomputed whenever
//! one of its inputs changes (an analysis, an edit, a setting). Edits are
//! kept in SONORA's database only: files on disk are never modified.
//!
//! Precedence: an edit (or accepted Discogs genre) first, then what the
//! analysis detected, then the tags; folder names only when the user turned
//! them on and nothing else names a genre.

use crate::db::sqlite::{Connection, SqlValue};
use crate::db::StoreError;
use crate::library::as_params;
use crate::params;
use sonora_engine::analysis::key::{camelot_order, parse_key};
use sonora_engine::analysis::tempo::resolve_bpm;
use std::path::Path;

/// Where a genre is taken from when the tags have none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FolderGenre {
    Off,
    /// The first folder inside the library folder ("Music/House/…" → House).
    Top,
    /// The folder the file is in.
    Parent,
}

impl FolderGenre {
    pub fn from_id(id: &str) -> Option<FolderGenre> {
        Some(match id {
            "off" => FolderGenre::Off,
            "top" => FolderGenre::Top,
            "parent" => FolderGenre::Parent,
            _ => return None,
        })
    }

    pub fn id(self) -> &'static str {
        match self {
            FolderGenre::Off => "off",
            FolderGenre::Top => "top",
            FolderGenre::Parent => "parent",
        }
    }
}

/// The settings that shape effective values.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DjConfig {
    /// Tempos are shown within this range (an octave or more wide).
    pub bpm_range: (f64, f64),
    pub folder_genre: FolderGenre,
}

impl Default for DjConfig {
    fn default() -> Self {
        DjConfig { bpm_range: sonora_engine::analysis::tempo::DEFAULT_RANGE, folder_genre: FolderGenre::Off }
    }
}

/// Serialises tempo candidates for the `tempo_candidates` column: "128:1;64:0.93".
pub fn candidates_to_column(c: &[(f64, f64)]) -> String {
    c.iter().map(|(b, s)| format!("{}:{}", trim_num(*b, 3), trim_num(*s, 3))).collect::<Vec<_>>().join(";")
}

pub fn candidates_from_column(s: &str) -> Vec<(f64, f64)> {
    s.split(';')
        .filter_map(|p| {
            let (b, sal) = p.split_once(':')?;
            Some((b.trim().parse().ok()?, sal.trim().parse().ok()?))
        })
        .collect()
}

fn trim_num(v: f64, d: usize) -> String {
    let s = format!("{v:.d$}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-" {
        "0".into()
    } else {
        s.to_string()
    }
}

/// A genre from the folder structure, or `None`.
pub fn folder_genre(path: &str, root: Option<&str>, mode: FolderGenre) -> Option<String> {
    let parent = Path::new(path).parent()?;
    let name = match mode {
        FolderGenre::Off => return None,
        FolderGenre::Parent => {
            // The library folder's own name is not a genre.
            if root.is_some_and(|r| Path::new(r) == parent) {
                return None;
            }
            parent.file_name()?.to_string_lossy().to_string()
        }
        FolderGenre::Top => {
            let rel = parent.strip_prefix(Path::new(root?)).ok()?;
            rel.components().next()?.as_os_str().to_string_lossy().to_string()
        }
    };
    let name = name.trim().to_string();
    (!name.is_empty()).then_some(name)
}

/// Rounds a tempo for storage (three decimals).
fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

struct Inputs {
    id: i64,
    tag_bpm: Option<f64>,
    tag_key: Option<String>,
    tag_genre: Option<String>,
    bpm_detected: Option<f64>,
    candidates: Option<String>,
    key_detected: Option<i64>,
    bpm_edit: Option<f64>,
    key_edit: Option<i64>,
    genre_edit: Option<String>,
    genre_edit_source: Option<String>,
    path: String,
    root: Option<String>,
}

const INPUT_SQL: &str = "SELECT t.id, t.bpm, t.musical_key, t.genre, t.bpm_detected, t.tempo_candidates, t.key_detected, t.bpm_edit, t.key_edit, \
t.genre_edit, t.genre_edit_source, t.path, f.path FROM tracks t LEFT JOIN folders f ON f.id = t.folder_id";

fn read_inputs(r: &crate::db::sqlite::Row) -> Inputs {
    Inputs {
        id: r.i64(0),
        tag_bpm: r.opt_f64(1),
        tag_key: r.opt_text(2),
        tag_genre: r.opt_text(3),
        bpm_detected: r.opt_f64(4),
        candidates: r.opt_text(5),
        key_detected: r.opt_i64(6),
        bpm_edit: r.opt_f64(7),
        key_edit: r.opt_i64(8),
        genre_edit: r.opt_text(9),
        genre_edit_source: r.opt_text(10),
        path: r.text(11),
        root: r.opt_text(12),
    }
}

struct Effective {
    bpm: Option<f64>,
    bpm_source: Option<&'static str>,
    key: Option<u8>,
    key_source: Option<&'static str>,
    genre: Option<String>,
    genre_source: Option<&'static str>,
}

fn effective(i: &Inputs, cfg: &DjConfig) -> Effective {
    let (lo, hi) = cfg.bpm_range;
    let (bpm, bpm_source) = if let Some(b) = i.bpm_edit {
        (Some(b), Some("edit"))
    } else if let Some(b) = i.bpm_detected {
        let candidates = i.candidates.as_deref().map(candidates_from_column).unwrap_or_default();
        (Some(round3(resolve_bpm(b, &candidates, lo, hi))), Some("detected"))
    } else if let Some(b) = i.tag_bpm {
        (Some(b), Some("tag"))
    } else {
        (None, None)
    };
    let valid = |k: i64| (0..24).contains(&k).then_some(k as u8);
    let (key, key_source) = if let Some(k) = i.key_edit.and_then(valid) {
        (Some(k), Some("edit"))
    } else if let Some(k) = i.key_detected.and_then(valid) {
        (Some(k), Some("detected"))
    } else if let Some(k) = i.tag_key.as_deref().and_then(parse_key) {
        (Some(k), Some("tag"))
    } else {
        (None, None)
    };
    let nonempty = |s: &Option<String>| s.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let (genre, genre_source) = if let Some(g) = nonempty(&i.genre_edit) {
        (Some(g), Some(if i.genre_edit_source.as_deref() == Some("discogs") { "discogs" } else { "edit" }))
    } else if let Some(g) = nonempty(&i.tag_genre) {
        (Some(g), Some("tag"))
    } else if let Some(g) = folder_genre(&i.path, i.root.as_deref(), cfg.folder_genre) {
        (Some(g), Some("folder"))
    } else {
        (None, None)
    };
    Effective { bpm, bpm_source, key, key_source, genre, genre_source }
}

/// Recomputes the effective DJ values of the given tracks (all when `None`)
/// in one transaction. Returns the number of tracks updated.
pub fn refresh(c: &Connection, ids: Option<&[i64]>, cfg: &DjConfig) -> Result<u64, StoreError> {
    c.transaction(|tx| refresh_within(tx, ids, cfg))
}

/// [`refresh`] for a caller that already holds a transaction.
pub fn refresh_within(c: &Connection, ids: Option<&[i64]>, cfg: &DjConfig) -> Result<u64, StoreError> {
    let mut inputs = Vec::new();
    match ids {
        None => inputs = c.query(INPUT_SQL, params![], read_inputs)?,
        Some(ids) => {
            for chunk in ids.chunks(500) {
                let marks = vec!["?"; chunk.len()].join(",");
                let p: Vec<SqlValue> = chunk.iter().map(|&i| SqlValue::Int(i)).collect();
                inputs.extend(c.query(&format!("{INPUT_SQL} WHERE t.id IN ({marks})"), &as_params(&p), read_inputs)?);
            }
        }
    }
    for i in &inputs {
        let e = effective(i, cfg);
        c.execute(
            "UPDATE tracks SET dj_bpm = ?, dj_bpm_source = ?, dj_key = ?, dj_key_source = ?, dj_key_order = ?, dj_genre = ?, dj_genre_source = ? WHERE id = ?",
            params![e.bpm, e.bpm_source, e.key.map(|k| k as i64), e.key_source, e.key.map(|k| camelot_order(k) as i64), e.genre, e.genre_source, i.id],
        )?;
    }
    Ok(inputs.len() as u64)
}

/// A change to the DJ values of some tracks. `None` leaves a field alone,
/// `Some(None)` removes the edit (back to the detected or tagged value).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Edit {
    pub bpm: Option<Option<f64>>,
    /// Multiplies the tempo now shown (0.5 or 2: half or double time).
    pub bpm_factor: Option<f64>,
    pub key: Option<Option<u8>>,
    pub genre: Option<Option<String>>,
    /// Removes every edit, including Discogs genres.
    pub reset: bool,
}

pub const MAX_GENRE_CHARS: usize = 80;

impl Edit {
    pub fn from_json(v: &sonora_json::Value) -> Result<Edit, String> {
        use sonora_json::Value;
        let mut e = Edit { reset: v.get("reset").and_then(Value::as_bool).unwrap_or(false), ..Default::default() };
        match v.get("bpm") {
            None => {}
            Some(Value::Null) => e.bpm = Some(None),
            Some(x) => {
                let b = x.as_f64().ok_or("bpm must be a number or null")?;
                if !(20.0..=400.0).contains(&b) {
                    return Err("bpm must be between 20 and 400".into());
                }
                e.bpm = Some(Some(round3(b)));
            }
        }
        if let Some(f) = v.get("bpmFactor").filter(|f| !f.is_null()) {
            let f = f.as_f64().ok_or("bpmFactor must be a number")?;
            if f != 0.5 && f != 2.0 {
                return Err("bpmFactor must be 0.5 or 2".into());
            }
            e.bpm_factor = Some(f);
        }
        match v.get("key") {
            None => {}
            Some(Value::Null) => e.key = Some(None),
            Some(x) => {
                let k = match x {
                    Value::String(s) => parse_key(s).ok_or_else(|| format!("'{s}' is not a key"))?,
                    other => other.as_i64().filter(|k| (0..24).contains(k)).ok_or("key must be 0–23, a key name or null")? as u8,
                };
                e.key = Some(Some(k));
            }
        }
        match v.get("genre") {
            None => {}
            Some(Value::Null) => e.genre = Some(None),
            Some(x) => {
                let g = x.as_str().ok_or("genre must be text or null")?.trim();
                if g.chars().count() > MAX_GENRE_CHARS {
                    return Err(format!("genre must be at most {MAX_GENRE_CHARS} characters"));
                }
                if g.chars().any(char::is_control) {
                    return Err("genre must not contain control characters".into());
                }
                e.genre = Some(if g.is_empty() { None } else { Some(g.to_string()) });
            }
        }
        if e.bpm.is_some() && e.bpm_factor.is_some() {
            return Err("give either bpm or bpmFactor".into());
        }
        Ok(e)
    }

    pub fn is_empty(&self) -> bool {
        !self.reset && self.bpm.is_none() && self.bpm_factor.is_none() && self.key.is_none() && self.genre.is_none()
    }
}

/// Applies an edit to tracks and refreshes their effective values.
pub fn apply_edit(c: &Connection, ids: &[i64], edit: &Edit, cfg: &DjConfig) -> Result<u64, StoreError> {
    c.transaction(|tx| {
        for &id in ids {
            if edit.reset {
                tx.execute("UPDATE tracks SET bpm_edit = NULL, key_edit = NULL, genre_edit = NULL, genre_edit_source = NULL, discogs_url = NULL WHERE id = ?", params![id])?;
            }
            if let Some(b) = edit.bpm {
                tx.execute("UPDATE tracks SET bpm_edit = ? WHERE id = ?", params![b, id])?;
            }
            if let Some(f) = edit.bpm_factor {
                // Doubles or halves what is shown now, whatever its source.
                tx.execute("UPDATE tracks SET bpm_edit = ROUND(dj_bpm * ?, 3) WHERE id = ? AND dj_bpm IS NOT NULL", params![f, id])?;
            }
            if let Some(k) = edit.key {
                tx.execute("UPDATE tracks SET key_edit = ? WHERE id = ?", params![k.map(|k| k as i64), id])?;
            }
            if let Some(g) = &edit.genre {
                tx.execute("UPDATE tracks SET genre_edit = ?, genre_edit_source = ?, discogs_url = NULL WHERE id = ?", params![g.clone(), g.as_ref().map(|_| "manual"), id])?;
            }
        }
        refresh_within(tx, Some(ids), cfg)
    })
}

/// Stores a genre found on Discogs as the track's genre (an edit the user
/// can undo), with the release it came from.
pub fn set_discogs_genre(c: &Connection, id: i64, genre: &str, url: &str, cfg: &DjConfig) -> Result<(), StoreError> {
    c.transaction(|tx| {
        tx.execute("UPDATE tracks SET genre_edit = ?, genre_edit_source = 'discogs', discogs_url = ? WHERE id = ?", params![genre, url, id])?;
        refresh_within(tx, Some(&[id]), cfg)?;
        Ok::<(), StoreError>(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_column_round_trip() {
        let c = vec![(128.0, 1.0), (64.0, 0.931), (85.333, 0.5)];
        let s = candidates_to_column(&c);
        assert_eq!(s, "128:1;64:0.931;85.333:0.5");
        assert_eq!(candidates_from_column(&s), c);
        assert!(candidates_from_column("").is_empty());
        assert_eq!(candidates_from_column("x:1;120:0.5"), vec![(120.0, 0.5)]);
    }

    #[test]
    fn genres_from_folders() {
        let root = if cfg!(windows) { r"C:\Music\DJ" } else { "/Music/DJ" };
        let sep = std::path::MAIN_SEPARATOR;
        let file = format!("{root}{sep}House{sep}Deep{sep}a.flac");
        assert_eq!(folder_genre(&file, Some(root), FolderGenre::Top).as_deref(), Some("House"));
        assert_eq!(folder_genre(&file, Some(root), FolderGenre::Parent).as_deref(), Some("Deep"));
        assert_eq!(folder_genre(&file, Some(root), FolderGenre::Off), None);
        let loose = format!("{root}{sep}a.flac");
        assert_eq!(folder_genre(&loose, Some(root), FolderGenre::Top), None, "a file directly in the library folder");
        assert_eq!(folder_genre(&loose, Some(root), FolderGenre::Parent), None, "the library folder itself is not a genre");
        assert_eq!(folder_genre(&file, None, FolderGenre::Top), None);
    }

    #[test]
    fn edits_parse_and_validate() {
        let p = |s: &str| Edit::from_json(&sonora_json::parse(s).unwrap());
        let e = p(r#"{"bpm": 128.0004, "key": "8A", "genre": "  Tech House "}"#).unwrap();
        assert_eq!(e.bpm, Some(Some(128.0)));
        assert_eq!(e.key, Some(Some(parse_key("Am").unwrap())));
        assert_eq!(e.genre, Some(Some("Tech House".into())));
        let clear = p(r#"{"bpm": null, "key": null, "genre": ""}"#).unwrap();
        assert_eq!((clear.bpm, clear.key, clear.genre), (Some(None), Some(None), Some(None)));
        assert!(p("{}").unwrap().is_empty());
        assert!(p(r#"{"bpm": 5}"#).is_err());
        assert!(p(r#"{"key": 24}"#).is_err());
        assert!(p(r#"{"key": "H#"}"#).is_err());
        assert!(p(r#"{"bpmFactor": 3}"#).is_err());
        assert!(p(r#"{"bpm": 120, "bpmFactor": 2}"#).is_err());
        assert!(p(&format!(r#"{{"genre": "{}"}}"#, "x".repeat(81))).is_err());
        assert_eq!(p(r#"{"key": 21}"#).unwrap().key, Some(Some(21)));
    }
}
