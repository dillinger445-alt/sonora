//! File inspection: container/codec identification and tag reading.
//!
//! Facts come from native header parsing (WAV, AIFF, FLAC, MPEG frame headers
//! and ID3, MP4/M4A atoms, Ogg Vorbis/Opus headers). ffprobe is consulted only
//! for containers the native parsers do not understand, and is optional.
//! Declared values are recorded as declared; the decoded audio is the
//! authority for duration and content.

pub mod id3;
pub mod mp4;
pub mod mpeg;
pub mod ogg;
pub mod vorbis_comment;

use crate::decode::sniff::{read_up_to, sniff_file, Container};
use crate::decode::{aiff, ffmpeg, flac, wav, ToolPaths};
use crate::error::AnalysisError;
use sonora_json::{obj, opt_num, ToJson, Value};
use std::fs::File;
use std::io::{Seek, SeekFrom};
use std::path::Path;

/// Descriptive tags. Missing titles fall back to the file name in the UI.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub genre: Option<String>,
    pub date: Option<String>,
    pub bpm: Option<f64>,
    pub key: Option<String>,
    pub comment: Option<String>,
}

impl Tags {
    fn from_pairs(pairs: &[(String, String)]) -> Tags {
        let get = |names: &[&str]| -> Option<String> {
            names.iter().find_map(|n| pairs.iter().find(|(k, _)| k == n).map(|(_, v)| v.trim().to_string())).filter(|s| !s.is_empty())
        };
        Tags {
            title: get(&["title"]),
            artist: get(&["artist", "album_artist", "albumartist", "performer"]),
            album: get(&["album"]),
            genre: get(&["genre"]),
            date: get(&["date", "year", "originaldate"]),
            bpm: get(&["bpm", "tbpm", "tempo"]).and_then(|s| s.replace(',', ".").trim().parse::<f64>().ok()).filter(|b| *b > 0.0 && *b < 1000.0),
            key: get(&["key", "initialkey", "initial_key", "tkey"]),
            comment: get(&["comment", "description"]),
        }
    }

    fn fill_missing(&mut self, other: Tags) {
        if self.title.is_none() {
            self.title = other.title;
        }
        if self.artist.is_none() {
            self.artist = other.artist;
        }
        if self.album.is_none() {
            self.album = other.album;
        }
        if self.genre.is_none() {
            self.genre = other.genre;
        }
        if self.date.is_none() {
            self.date = other.date;
        }
        if self.bpm.is_none() {
            self.bpm = other.bpm;
        }
        if self.key.is_none() {
            self.key = other.key;
        }
        if self.comment.is_none() {
            self.comment = other.comment;
        }
    }
}

impl ToJson for Tags {
    fn to_json(&self) -> Value {
        obj! {
            "title" => self.title, "artist" => self.artist, "album" => self.album,
            "genre" => self.genre, "date" => self.date, "bpm" => opt_num(self.bpm, 2),
            "key" => self.key, "comment" => self.comment,
        }
    }
}

/// Facts about the file and its declared encoding.
#[derive(Clone, Debug)]
pub struct FileFacts {
    pub file_name: String,
    pub extension: String,
    pub file_size: u64,
    pub container: Container,
    pub container_name: String,
    pub codec_id: String,
    pub codec_name: String,
    pub codec_profile: Option<String>,
    pub lossless: Option<bool>,
    pub float_samples: bool,
    pub sample_rate: u32,
    pub channels: u16,
    pub channel_layout: Option<String>,
    pub bit_depth: Option<u16>,
    pub bitrate_kbps: Option<f64>,
    pub bitrate_mode: Option<String>,
    pub bitrate_mode_source: Option<String>,
    pub header_duration_secs: Option<f64>,
    pub tags: Tags,
    pub encoder: Option<String>,
    pub encoder_lowpass_hz: Option<u32>,
    pub extension_mismatch: Option<String>,
    pub metadata_source: String,
    /// Codec-level diagnostic: the container claims more audio than exists.
    pub header_truncated: bool,
}

impl ToJson for FileFacts {
    fn to_json(&self) -> Value {
        obj! {
            "fileName" => self.file_name,
            "extension" => self.extension,
            "fileSize" => self.file_size,
            "container" => self.container.id(),
            "containerName" => self.container_name,
            "codecId" => self.codec_id,
            "codecName" => self.codec_name,
            "codecProfile" => self.codec_profile,
            "lossless" => self.lossless,
            "floatSamples" => self.float_samples,
            "sampleRate" => self.sample_rate,
            "channels" => self.channels,
            "channelLayout" => self.channel_layout,
            "bitDepth" => self.bit_depth,
            "bitrateKbps" => opt_num(self.bitrate_kbps, 1),
            "bitrateMode" => self.bitrate_mode,
            "bitrateModeSource" => self.bitrate_mode_source,
            "headerDurationSecs" => opt_num(self.header_duration_secs, 3),
            "tags" => self.tags,
            "encoder" => self.encoder,
            "encoderLowpassHz" => self.encoder_lowpass_hz,
            "extensionMismatch" => self.extension_mismatch,
            "metadataSource" => self.metadata_source,
            "headerTruncated" => self.header_truncated,
        }
    }
}

impl Tags {
    fn from_json(v: Option<&Value>) -> Tags {
        let Some(v) = v.filter(|v| !v.is_null()) else { return Tags::default() };
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        Tags {
            title: s("title"),
            artist: s("artist"),
            album: s("album"),
            genre: s("genre"),
            date: s("date"),
            bpm: v.get("bpm").and_then(Value::as_f64),
            key: s("key"),
            comment: s("comment"),
        }
    }
}

impl FileFacts {
    /// Parses a record produced by `to_json`, so stored analyses can be
    /// re-scored without decoding the audio again.
    pub fn from_json(v: &Value) -> Option<FileFacts> {
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        let u = |k: &str| v.get(k).and_then(Value::as_u64);
        Some(FileFacts {
            file_name: s("fileName")?,
            extension: s("extension").unwrap_or_default(),
            file_size: u("fileSize").unwrap_or(0),
            container: Container::from_id(v.get("container").and_then(Value::as_str).unwrap_or("unknown")),
            container_name: s("containerName").unwrap_or_default(),
            codec_id: s("codecId")?,
            codec_name: s("codecName").unwrap_or_default(),
            codec_profile: s("codecProfile"),
            lossless: v.get("lossless").and_then(Value::as_bool),
            float_samples: v.get("floatSamples").and_then(Value::as_bool).unwrap_or(false),
            sample_rate: u("sampleRate")? as u32,
            channels: u("channels")? as u16,
            channel_layout: s("channelLayout"),
            bit_depth: u("bitDepth").map(|b| b as u16),
            bitrate_kbps: v.get("bitrateKbps").and_then(Value::as_f64),
            bitrate_mode: s("bitrateMode"),
            bitrate_mode_source: s("bitrateModeSource"),
            header_duration_secs: v.get("headerDurationSecs").and_then(Value::as_f64),
            tags: Tags::from_json(v.get("tags")),
            encoder: s("encoder"),
            encoder_lowpass_hz: u("encoderLowpassHz").map(|x| x as u32),
            extension_mismatch: s("extensionMismatch"),
            metadata_source: s("metadataSource").unwrap_or_default(),
            header_truncated: v.get("headerTruncated").and_then(Value::as_bool).unwrap_or(false),
        })
    }
}

/// Lossless status of an FFmpeg codec id.
pub fn codec_is_lossless(codec_id: &str) -> Option<bool> {
    // A-law and µ-law are logarithmic 8-bit companding: lossy, despite the pcm_ name.
    if matches!(codec_id, "pcm_alaw" | "pcm_mulaw" | "pcm_vidc") {
        return Some(false);
    }
    if codec_id.starts_with("pcm_") {
        return Some(true);
    }
    match codec_id {
        "flac" | "alac" | "wavpack" | "ape" | "tta" | "mlp" | "truehd" | "shorten" | "wmalossless" | "pcm" | "pcm_float" => Some(true),
        "mp3" | "mp3float" | "mp2" | "mp1" | "aac" | "aac_latm" | "vorbis" | "opus" | "wmav1" | "wmav2" | "wmapro" | "ac3" | "eac3" | "dts" | "amr_nb"
        | "amr_wb" | "speex" | "gsm" | "adpcm_ms" | "adpcm_ima_wav" | "adpcm_ima" | "alaw" | "mulaw" | "pcm_alaw" | "pcm_mulaw" | "musepack7"
        | "musepack8" | "atrac3" | "ra_144" | "cook" => Some(false),
        _ => None,
    }
}

fn codec_display(codec_id: &str, profile: Option<&str>, bits: Option<u16>, float: bool, long_name: &str) -> String {
    if codec_id.starts_with("pcm_") || codec_id == "pcm" || codec_id == "pcm_float" {
        if float {
            return format!("PCM {}-bit float", bits.unwrap_or(32));
        }
        return match bits {
            Some(b) => format!("PCM {b}-bit"),
            None => "PCM".to_string(),
        };
    }
    match codec_id {
        "flac" => "FLAC".into(),
        "alac" => "ALAC (Apple Lossless)".into(),
        "mp3" | "mp3float" => "MP3".into(),
        "mp2" => "MPEG Layer II".into(),
        "aac" => match profile {
            Some(p) if p.contains("HE-AACv2") => "HE-AAC v2".into(),
            Some(p) if p.contains("HE") => "HE-AAC".into(),
            Some(p) if p.eq_ignore_ascii_case("LC") => "AAC-LC".into(),
            _ => "AAC".into(),
        },
        "vorbis" => "Vorbis".into(),
        "opus" => "Opus".into(),
        "wavpack" => "WavPack".into(),
        "ape" => "Monkey's Audio".into(),
        "wmav2" | "wmav1" => "WMA".into(),
        "wmapro" => "WMA Pro".into(),
        "wmalossless" => "WMA Lossless".into(),
        _ if !long_name.is_empty() => long_name.to_string(),
        other => other.to_uppercase(),
    }
}

fn expected_containers(ext: &str) -> &'static [Container] {
    match ext {
        "wav" | "wave" | "bwf" => &[Container::Wav],
        "aif" | "aiff" | "aifc" => &[Container::Aiff],
        "flac" => &[Container::Flac],
        "mp3" | "mp2" => &[Container::Mp3],
        "m4a" | "mp4" | "alac" | "m4b" => &[Container::Mp4],
        "aac" => &[Container::AacAdts, Container::Mp4],
        "ogg" | "oga" | "opus" => &[Container::Ogg, Container::Matroska],
        "webm" | "mka" => &[Container::Matroska],
        "wv" => &[Container::WavPack],
        "ape" => &[Container::Ape],
        "caf" => &[Container::Caf],
        "wma" => &[Container::Asf],
        _ => &[],
    }
}

/// Inspects `path` without decoding audio.
pub fn probe_file(path: &Path, tools: &ToolPaths) -> Result<FileFacts, AnalysisError> {
    let meta = std::fs::metadata(path)?;
    if !meta.is_file() {
        return Err(AnalysisError::Io("not a regular file".into()));
    }
    let file_size = meta.len();
    if file_size == 0 {
        return Err(AnalysisError::Corrupt("file is empty".into()));
    }
    let file_name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let extension = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let container = sniff_file(path)?;

    let mut facts = FileFacts {
        file_name,
        extension: extension.clone(),
        file_size,
        container,
        container_name: container.display_name().to_string(),
        codec_id: String::new(),
        codec_name: String::new(),
        codec_profile: None,
        lossless: None,
        float_samples: false,
        sample_rate: 0,
        channels: 0,
        channel_layout: None,
        bit_depth: None,
        bitrate_kbps: None,
        bitrate_mode: None,
        bitrate_mode_source: None,
        header_duration_secs: None,
        tags: Tags::default(),
        encoder: None,
        encoder_lowpass_hz: None,
        extension_mismatch: None,
        metadata_source: "native".into(),
        header_truncated: false,
    };

    let mut native_tags: Vec<(String, String)> = Vec::new();
    let mut native_ok = false;
    match container {
        Container::Wav => {
            let h = match wav::parse_header(path) {
                Ok(h) => h,
                Err(wav::OpenError::NotNative(h)) => h,
                Err(wav::OpenError::Failed(e)) => return Err(e),
            };
            facts.container_name = if h.rf64 { "WAV (RF64)".into() } else { "WAV".into() };
            facts.sample_rate = h.sample_rate;
            facts.channels = h.channels;
            facts.header_truncated = h.truncated;
            // ID3 first: it states its text encoding, INFO does not.
            if let Some((off, len)) = h.id3_chunk {
                native_tags.extend(read_id3_at(path, off, len));
            }
            native_tags.extend(h.info_tags.iter().cloned());
            if let Some(enc) = h.encoding {
                native_ok = true;
                facts.float_samples = enc.is_float();
                let bits = if enc.is_float() { enc.container_bits() } else { h.valid_bits.unwrap_or(h.bits_per_sample.min(enc.container_bits())) };
                facts.bit_depth = Some(bits);
                facts.codec_id = if enc.is_float() { format!("pcm_f{}le", enc.container_bits()) } else { format!("pcm_s{}le", enc.container_bits()) };
                facts.lossless = Some(true);
                facts.bitrate_kbps = Some(h.sample_rate as f64 * h.block_align as f64 * 8.0 / 1000.0);
                facts.bitrate_mode = Some("CBR".into());
                facts.header_duration_secs = h.declared_frames().map(|f| f as f64 / h.sample_rate as f64);
            } else {
                facts.codec_id = h.codec_name().to_string();
                facts.lossless = codec_is_lossless(h.codec_name());
            }
        }
        Container::Aiff => {
            let h = match aiff::parse_header(path) {
                Ok(h) => h,
                Err(aiff::OpenError::NotNative(h)) => h,
                Err(aiff::OpenError::Failed(e)) => return Err(e),
            };
            facts.container_name = if h.aifc { "AIFF-C".into() } else { "AIFF".into() };
            facts.sample_rate = h.sample_rate.round() as u32;
            facts.channels = h.channels;
            facts.header_truncated = h.truncated;
            facts.header_duration_secs = Some(h.frames as f64 / h.sample_rate);
            if let Some((off, len)) = h.id3_chunk {
                native_tags.extend(read_id3_at(path, off, len));
            }
            native_tags.extend(h.text_tags.iter().cloned());
            if let Some(enc) = h.encoding {
                native_ok = true;
                facts.float_samples = enc.is_float();
                facts.bit_depth = Some(if enc.is_float() { enc.container_bits() } else { h.sample_size });
                facts.codec_id = if enc.is_float() { format!("pcm_f{}be", enc.container_bits()) } else { format!("pcm_s{}be", enc.container_bits()) };
                facts.lossless = Some(true);
                facts.bitrate_kbps = Some(h.sample_rate * h.channels as f64 * enc.bytes() as f64 * 8.0 / 1000.0);
                facts.bitrate_mode = Some("CBR".into());
            } else {
                let c = h.compression_str().to_ascii_lowercase();
                facts.codec_id = match c.as_str() {
                    "ulaw" => "pcm_mulaw".into(),
                    "alaw" => "pcm_alaw".into(),
                    other => other.to_string(),
                };
                facts.lossless = codec_is_lossless(&facts.codec_id);
            }
        }
        Container::Mp3 => {
            if let Some(m) = mpeg::parse_file(path) {
                facts.codec_id = match m.layer {
                    3 => "mp3".into(),
                    2 => "mp2".into(),
                    _ => "mp1".into(),
                };
                facts.codec_profile = Some(format!("{} Layer {}", m.version, ["I", "II", "III"][(m.layer - 1) as usize]));
                facts.lossless = Some(false);
                facts.sample_rate = m.sample_rate;
                facts.channels = m.channels;
                facts.channel_layout = Some(m.channel_mode.to_string());
                facts.bitrate_mode = m.bitrate_mode.map(str::to_string);
                facts.bitrate_mode_source = Some(m.mode_source.to_string()).filter(|s| s != "unknown");
                facts.bitrate_kbps = m.avg_bitrate_kbps;
                facts.encoder = m.encoder.clone();
                facts.encoder_lowpass_hz = m.lowpass_hz;
                // The declared length, so an MP3 cut short (an incomplete
                // download) is found truncated (B33).
                facts.header_duration_secs = m.declared_secs();
                native_ok = true;
            }
            native_tags.extend(read_leading_id3(path));
        }
        Container::Flac => {
            if let Ok(h) = flac::parse_header(path) {
                if h.sample_rate > 0 && h.channels > 0 {
                    native_ok = true;
                    facts.codec_id = "flac".into();
                    facts.lossless = Some(true);
                    facts.sample_rate = h.sample_rate;
                    facts.channels = h.channels;
                    facts.bit_depth = Some(h.bits_per_sample);
                    facts.header_duration_secs = h.total_frames.map(|f| f as f64 / h.sample_rate as f64);
                    if let Some(d) = facts.header_duration_secs.filter(|d| *d > 0.0) {
                        facts.bitrate_kbps = Some(file_size.saturating_sub(h.audio_offset) as f64 * 8.0 / d / 1000.0);
                    }
                    facts.bitrate_mode = Some("VBR".into());
                    facts.bitrate_mode_source = Some("format".into());
                    facts.encoder = h.vendor.clone();
                    native_tags.extend(h.tags.iter().cloned());
                }
            }
            native_tags.extend(read_leading_id3(path));
        }
        Container::Mp4 => {
            if let Ok(m) = mp4::parse_file(path) {
                if m.sample_rate > 0 && m.channels > 0 && !m.codec_id.is_empty() {
                    native_ok = true;
                    facts.container_name = match m.brand.as_deref().map(str::trim) {
                        Some("M4A") | Some("M4B") | Some("M4P") => "M4A".into(),
                        _ => "MP4".into(),
                    };
                    facts.codec_id = m.codec_id.clone();
                    facts.codec_profile = m.codec_profile.clone();
                    facts.sample_rate = m.sample_rate;
                    facts.channels = m.channels;
                    facts.lossless = codec_is_lossless(&m.codec_id);
                    if facts.lossless == Some(true) {
                        facts.bit_depth = m.bits_per_sample;
                    }
                    facts.header_duration_secs = m.duration_secs;
                    facts.bitrate_kbps = m.avg_bitrate_kbps;
                    if let (Some(avg), Some(max)) = (m.avg_bitrate_kbps, m.max_bitrate_kbps) {
                        if facts.lossless == Some(false) && max > 0.0 && avg > 0.0 {
                            facts.bitrate_mode = Some(if (max - avg).abs() / avg < 0.02 { "CBR".into() } else { "VBR".into() });
                            facts.bitrate_mode_source = Some("esds".into());
                        }
                    }
                    facts.encoder = m.encoder.clone();
                    native_tags.extend(m.tags.iter().cloned());
                }
            }
        }
        Container::Ogg => {
            if let Ok(o) = ogg::parse_file(path) {
                if o.sample_rate > 0 && o.channels > 0 && !o.codec_id.is_empty() {
                    native_ok = true;
                    facts.container_name = match o.codec_id.as_str() {
                        "opus" => "Ogg Opus".into(),
                        "vorbis" => "Ogg Vorbis".into(),
                        "flac" => "Ogg FLAC".into(),
                        _ => "Ogg".into(),
                    };
                    facts.codec_id = o.codec_id.clone();
                    facts.sample_rate = o.sample_rate;
                    facts.channels = o.channels;
                    facts.lossless = codec_is_lossless(&o.codec_id);
                    if facts.lossless == Some(true) {
                        facts.bit_depth = o.bits_per_sample;
                    }
                    facts.header_duration_secs = o.duration_secs;
                    facts.bitrate_kbps = o.avg_bitrate_kbps.or(o.nominal_bitrate_kbps);
                    facts.encoder = o.vendor.clone();
                    native_tags.extend(o.tags.iter().cloned());
                }
            }
        }
        _ => {
            native_tags.extend(read_leading_id3(path));
        }
    }

    // ffprobe is only a fallback for containers the native parsers cannot read.
    let mut probe_ok = false;
    if !native_ok {
        if let Some(ffprobe) = &tools.ffprobe {
            if let Ok(p) = ffmpeg::probe(ffprobe, path) {
                if let Some(s) = &p.stream {
                    probe_ok = true;
                    facts.metadata_source = "ffprobe".into();
                    facts.codec_id = if s.codec_name == "mp3float" { "mp3".into() } else { s.codec_name.clone() };
                    facts.sample_rate = s.sample_rate;
                    facts.channels = s.channels;
                    facts.channel_layout = s.channel_layout.clone();
                    facts.codec_profile = s.profile.clone();
                    facts.lossless = codec_is_lossless(&facts.codec_id);
                    if facts.lossless == Some(true) {
                        facts.bit_depth = s.bits_per_raw_sample.or(if s.bits_per_sample > 0 { Some(s.bits_per_sample) } else { None });
                    }
                    // Float samples in a lossless codec (float PCM, float WavPack…). Lossy
                    // decoders also report float sample formats; that says nothing about the source.
                    if (s.sample_fmt.starts_with("flt") || s.sample_fmt.starts_with("dbl")) && facts.lossless == Some(true) {
                        facts.float_samples = true;
                    }
                    facts.bitrate_kbps = s.bit_rate.or(p.bit_rate).map(|b| b / 1000.0);
                    facts.header_duration_secs = s.duration.or(p.duration);
                    facts.container_name = container_from_format(&p.format_name, container).to_string();
                }
                let ft = Tags::from_pairs(&p.tags);
                facts.tags = ft;
                if facts.encoder.is_none() {
                    facts.encoder = p.tags.iter().find(|(k, _)| k == "encoder").map(|(_, v)| v.clone());
                }
            }
        }
    }
    facts.tags.fill_missing(Tags::from_pairs(&native_tags));

    if !native_ok && !probe_ok {
        if container == Container::Unknown && !tools.has_ffmpeg() {
            return Err(AnalysisError::Unsupported("content is not a recognised audio format".into()));
        }
        // FFmpeg can still identify and decode it; the decoded stream will
        // supply the sample rate and channel count.
        facts.metadata_source = "decoder".into();
    }
    if facts.codec_id.is_empty() {
        facts.codec_id = "unknown".into();
    }
    facts.codec_name = codec_display(&facts.codec_id, facts.codec_profile.as_deref(), facts.bit_depth, facts.float_samples, "");
    if facts.lossless == Some(false) {
        facts.bit_depth = None;
    }

    // Extension vs content.
    let expected = expected_containers(&extension);
    if !expected.is_empty() && container != Container::Unknown && !expected.contains(&container) {
        facts.extension_mismatch = Some(format!(
            "The .{extension} extension suggests {}, but the content is {}.",
            expected[0].display_name(),
            facts.container_name
        ));
    } else if container == Container::Wav && facts.lossless == Some(false) {
        facts.extension_mismatch = Some(format!("This WAV file contains {}-compressed audio, not PCM.", facts.codec_name));
    }
    Ok(facts)
}

fn container_from_format(format_name: &str, fallback: Container) -> &'static str {
    let f = format_name.to_ascii_lowercase();
    if f.contains("mp4") || f.contains("mov") {
        "MP4/M4A"
    } else if f.contains("ogg") {
        "Ogg"
    } else if f.contains("matroska") || f.contains("webm") {
        "Matroska/WebM"
    } else if f.contains("flac") {
        "FLAC"
    } else if f.contains("asf") {
        "ASF/WMA"
    } else if f.contains("wv") {
        "WavPack"
    } else if f.contains("ape") {
        "Monkey's Audio"
    } else if f.contains("caf") {
        "CAF"
    } else {
        fallback.display_name()
    }
}

fn read_leading_id3(path: &Path) -> Vec<(String, String)> {
    let mut f = match File::open(path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    let mut head = [0u8; 10];
    if read_up_to(&mut f, &mut head).unwrap_or(0) < 10 {
        return Vec::new();
    }
    match crate::decode::sniff::id3v2_size(&head) {
        Some(size) if size <= 16 * 1024 * 1024 => read_id3_at(path, 0, size),
        _ => Vec::new(),
    }
}

fn read_id3_at(path: &Path, offset: u64, len: u64) -> Vec<(String, String)> {
    if len > 16 * 1024 * 1024 {
        return Vec::new();
    }
    let mut f = match File::open(path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    if f.seek(SeekFrom::Start(offset)).is_err() {
        return Vec::new();
    }
    let mut buf = vec![0u8; len as usize];
    let n = read_up_to(&mut f, &mut buf).unwrap_or(0);
    id3::parse_tag(&buf[..n])
}
