//! Listening inside the app.
//!
//! The interface plays files with the web view's own audio element, which
//! reads the original file when the web view can decode it. Which formats
//! that is depends on the platform (WebKit on macOS plays AIFF and ALAC,
//! Chromium-based WebView2 on Windows does not; WebView2 plays Ogg, WebKit
//! may not), so the interface says what it can play and anything else is
//! decoded once into a WAV in SONORA's cache folder. Nothing is uploaded and
//! the original file is only read.

use crate::model::TrackRow;
use sonora_engine::decode::ToolPaths;
use sonora_engine::export::convert::{convert, ConvertRequest, Quality, Target};
use sonora_engine::export::id3::TagSet;
use sonora_engine::meta::probe_file;
use sonora_json::{obj, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

/// Decoded copies kept for playback (the newest ones).
const CACHE_FILES: usize = 6;

/// What the web view needs to know to play a file: the codec family and MIME type.
pub fn kind(t: &TrackRow) -> Option<(&'static str, &'static str)> {
    let codec = t.codec.as_deref().unwrap_or("");
    let container = t.container.as_deref().unwrap_or("").to_ascii_uppercase();
    Some(match codec {
        "mp3" => ("mp3", "audio/mpeg"),
        "aac" => ("aac", "audio/mp4"),
        "alac" => ("alac", "audio/mp4"),
        "flac" if container.starts_with("FLAC") => ("flac", "audio/flac"),
        "vorbis" if container.starts_with("OGG") => ("vorbis", "audio/ogg"),
        "opus" if container.starts_with("OGG") => ("opus", "audio/ogg"),
        // RF64 ("WAV (RF64)") is beyond what web views read.
        c if c.starts_with("pcm_f") && container == "WAV" => ("wavfloat", "audio/wav"),
        c if c.starts_with("pcm_") && !c.contains("law") && container == "WAV" => ("wav", "audio/wav"),
        c if c.starts_with("pcm_") && !c.contains("law") && container.starts_with("AIFF") => ("aiff", "audio/aiff"),
        _ => return None,
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct Prepared {
    pub path: PathBuf,
    pub mime: &'static str,
    pub transcoded: bool,
}

impl Prepared {
    pub fn to_json(&self) -> Value {
        obj! { "path" => self.path.to_string_lossy().to_string(), "mime" => self.mime, "transcoded" => self.transcoded }
    }
}

/// Keeps the newest decoded copies and removes the rest (only SONORA's own cache files).
fn trim_cache(dir: &Path, keep: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "wav") && p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("track-")))
        .filter_map(|p| Some((std::fs::metadata(&p).ok()?.modified().ok()?, p)))
        .collect();
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    for (_, p) in files.into_iter().skip(CACHE_FILES) {
        if p != keep {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// The file to play for `t`: the original when `playable` lists its kind,
/// otherwise a decoded WAV in `cache_dir` (made once, then reused).
pub fn prepare(cache_dir: &Path, t: &TrackRow, playable: &[String], force_decode: bool, tools: &ToolPaths) -> Result<Prepared, String> {
    let src = Path::new(&t.path);
    if t.missing_since.is_some() || !src.is_file() {
        return Err(format!("{} is no longer there", t.path));
    }
    if let Some((k, mime)) = kind(t) {
        if !force_decode && playable.iter().any(|p| p == k) {
            return Ok(Prepared { path: src.to_path_buf(), mime, transcoded: false });
        }
    }
    std::fs::create_dir_all(cache_dir).map_err(|e| format!("cannot create the playback cache: {e}"))?;
    let stamp = t.file_hash.as_deref().map(|h| h.chars().take(16).collect::<String>()).unwrap_or_else(|| format!("{}-{}", t.size, t.mtime));
    let dest = cache_dir.join(format!("track-{}-{stamp}.wav", t.id));
    if dest.is_file() {
        // Touch it so it counts as recently used.
        if let Ok(f) = std::fs::File::options().append(true).open(&dest) {
            let _ = f.set_modified(std::time::SystemTime::now());
        }
        return Ok(Prepared { path: dest, mime: "audio/wav", transcoded: true });
    }
    let facts = probe_file(src, tools).map_err(|e| e.to_string())?;
    let tmp = cache_dir.join(format!("track-{}-{stamp}.{}.part", t.id, std::process::id()));
    let cancel = AtomicBool::new(false);
    let tags = TagSet::default();
    let req = ConvertRequest { src, facts: &facts, target: Target::Wav, quality: Quality::High, tags: &tags, dest: &tmp, tools, cancel: &cancel };
    match convert(&req, &mut |_| {}) {
        Ok(_) => {
            std::fs::rename(&tmp, &dest).map_err(|e| {
                let _ = std::fs::remove_file(&tmp);
                format!("cannot store the decoded copy: {e}")
            })?;
            trim_cache(cache_dir, &dest);
            Ok(Prepared { path: dest, mime: "audio/wav", transcoded: true })
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(format!("this file cannot be played: {e}"))
        }
    }
}
