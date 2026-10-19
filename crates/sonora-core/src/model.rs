//! Data shapes exchanged with the interface.

use crate::db::sqlite::Row;
use sonora_json::{num, obj, opt_num, ToJson, Value};

/// Analysis state of a track. Presence on disk is tracked separately
/// (`missing_since`), so a moved-away file keeps its results.
pub mod status {
    pub const PENDING: &str = "pending";
    pub const ANALYZING: &str = "analyzing";
    pub const DONE: &str = "done";
    pub const FAILED: &str = "failed";
}

/// Columns read by [`TrackRow::from_row`], in order.
pub const TRACK_COLUMNS: &str = "id, path, folder_id, file_name, ext, size, mtime, status, error_code, error_message, \
added_at, analyzed_at, missing_since, title, artist, album, genre, bpm, musical_key, container, codec, codec_name, \
format, lossless, sample_rate, bit_depth, bitrate_kbps, channels, duration, integrated_lufs, true_peak, cutoff_hz, upper_hz, \
overall, dj, listening, confidence, confidence_label, verdict, flags, model_version, file_hash, audio_hash";

/// One library row (no analysis payload).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrackRow {
    pub id: i64,
    pub path: String,
    pub folder_id: Option<i64>,
    pub file_name: String,
    pub ext: String,
    pub size: i64,
    pub mtime: i64,
    pub status: String,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub added_at: i64,
    pub analyzed_at: Option<i64>,
    pub missing_since: Option<i64>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub genre: Option<String>,
    pub bpm: Option<f64>,
    pub key: Option<String>,
    pub container: Option<String>,
    pub codec: Option<String>,
    pub codec_name: Option<String>,
    /// Display format: WAV, AIFF, FLAC, MP3, AAC, ALAC, Vorbis, Opus...
    pub format: Option<String>,
    pub lossless: Option<bool>,
    pub sample_rate: Option<i64>,
    pub bit_depth: Option<i64>,
    pub bitrate_kbps: Option<f64>,
    pub channels: Option<i64>,
    pub duration: Option<f64>,
    pub integrated_lufs: Option<f64>,
    pub true_peak: Option<f64>,
    pub cutoff_hz: Option<f64>,
    pub upper_hz: Option<f64>,
    pub overall: Option<f64>,
    pub dj: Option<f64>,
    pub listening: Option<f64>,
    pub confidence: Option<f64>,
    pub confidence_label: Option<String>,
    pub verdict: Option<String>,
    pub flags: Vec<String>,
    pub model_version: Option<String>,
    pub file_hash: Option<String>,
    pub audio_hash: Option<String>,
}

pub fn flags_to_column(flags: &[String]) -> String {
    if flags.is_empty() {
        String::new()
    } else {
        format!(",{},", flags.join(","))
    }
}

pub fn flags_from_column(s: &str) -> Vec<String> {
    s.split(',').filter(|f| !f.is_empty()).map(str::to_string).collect()
}

impl TrackRow {
    pub fn from_row(r: &Row) -> TrackRow {
        TrackRow {
            id: r.i64(0),
            path: r.text(1),
            folder_id: r.opt_i64(2),
            file_name: r.text(3),
            ext: r.text(4),
            size: r.i64(5),
            mtime: r.i64(6),
            status: r.text(7),
            error_code: r.opt_text(8),
            error_message: r.opt_text(9),
            added_at: r.i64(10),
            analyzed_at: r.opt_i64(11),
            missing_since: r.opt_i64(12),
            title: r.opt_text(13),
            artist: r.opt_text(14),
            album: r.opt_text(15),
            genre: r.opt_text(16),
            bpm: r.opt_f64(17),
            key: r.opt_text(18),
            container: r.opt_text(19),
            codec: r.opt_text(20),
            codec_name: r.opt_text(21),
            format: r.opt_text(22),
            lossless: r.opt_bool(23),
            sample_rate: r.opt_i64(24),
            bit_depth: r.opt_i64(25),
            bitrate_kbps: r.opt_f64(26),
            channels: r.opt_i64(27),
            duration: r.opt_f64(28),
            integrated_lufs: r.opt_f64(29),
            true_peak: r.opt_f64(30),
            cutoff_hz: r.opt_f64(31),
            upper_hz: r.opt_f64(32),
            overall: r.opt_f64(33),
            dj: r.opt_f64(34),
            listening: r.opt_f64(35),
            confidence: r.opt_f64(36),
            confidence_label: r.opt_text(37),
            verdict: r.opt_text(38),
            flags: flags_from_column(&r.text(39)),
            model_version: r.opt_text(40),
            file_hash: r.opt_text(41),
            audio_hash: r.opt_text(42),
        }
    }

    /// Title for display: the tag, or the file name without extension.
    pub fn display_title(&self) -> String {
        match &self.title {
            Some(t) if !t.trim().is_empty() => t.clone(),
            _ => std::path::Path::new(&self.file_name).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| self.file_name.clone()),
        }
    }
}

impl ToJson for TrackRow {
    fn to_json(&self) -> Value {
        obj! {
            "id" => self.id,
            "path" => self.path,
            "folderId" => self.folder_id,
            "fileName" => self.file_name,
            "ext" => self.ext,
            "size" => self.size,
            "mtime" => self.mtime,
            "status" => self.status,
            "errorCode" => self.error_code,
            "errorMessage" => self.error_message,
            "addedAt" => self.added_at,
            "analyzedAt" => self.analyzed_at,
            "missing" => self.missing_since.is_some(),
            "missingSince" => self.missing_since,
            "title" => self.title,
            "displayTitle" => self.display_title(),
            "artist" => self.artist,
            "album" => self.album,
            "genre" => self.genre,
            "bpm" => opt_num(self.bpm, 1),
            "key" => self.key,
            "container" => self.container,
            "codec" => self.codec,
            "codecName" => self.codec_name,
            "format" => self.format,
            "lossless" => self.lossless,
            "sampleRate" => self.sample_rate,
            "bitDepth" => self.bit_depth,
            "bitrateKbps" => opt_num(self.bitrate_kbps, 0),
            "channels" => self.channels,
            "duration" => opt_num(self.duration, 2),
            "integratedLufs" => opt_num(self.integrated_lufs, 1),
            "truePeak" => opt_num(self.true_peak, 2),
            "cutoffHz" => opt_num(self.cutoff_hz, 0),
            "upperHz" => opt_num(self.upper_hz, 0),
            "overall" => opt_num(self.overall, 0),
            "dj" => opt_num(self.dj, 0),
            "listening" => opt_num(self.listening, 0),
            "confidence" => opt_num(self.confidence, 2),
            "confidenceLabel" => self.confidence_label,
            "verdict" => self.verdict,
            "flags" => self.flags,
            "modelVersion" => self.model_version,
        }
    }
}

/// A rounded number or null (helper for summaries).
pub fn n(v: f64, d: u32) -> Value {
    num(v, d)
}

/// Display format for a codec/container pair.
pub fn format_label(codec: &str, container: &str) -> String {
    let c = codec.to_ascii_lowercase();
    if c.starts_with("pcm") {
        let k = container.to_ascii_lowercase();
        return if k.starts_with("aif") { "AIFF".into() } else if k.contains("mp4") || k.contains("m4a") { "PCM".into() } else { "WAV".into() };
    }
    match c.as_str() {
        "flac" => "FLAC".into(),
        "mp3" => "MP3".into(),
        "mp2" | "mp1" => "MP2".into(),
        "aac" => "AAC".into(),
        "alac" => "ALAC".into(),
        "vorbis" => "Vorbis".into(),
        "opus" => "Opus".into(),
        "wavpack" => "WavPack".into(),
        "ape" => "APE".into(),
        "wmav1" | "wmav2" | "wmapro" | "wmalossless" => "WMA".into(),
        "ac3" | "eac3" => "AC-3".into(),
        "" | "unknown" => "Unknown".into(),
        other => other.to_uppercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_column_roundtrip() {
        let f = vec!["clipping".to_string(), "lossy".to_string()];
        let c = flags_to_column(&f);
        assert_eq!(c, ",clipping,lossy,");
        assert_eq!(flags_from_column(&c), f);
        assert!(flags_from_column("").is_empty());
    }

    #[test]
    fn display_title_falls_back_to_file_name() {
        let t = TrackRow { file_name: "Artist - Track (Extended Mix).flac".into(), ..Default::default() };
        assert_eq!(t.display_title(), "Artist - Track (Extended Mix)");
        let t2 = TrackRow { title: Some("Real Title".into()), ..t };
        assert_eq!(t2.display_title(), "Real Title");
    }
}
