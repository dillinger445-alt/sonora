//! Native MP4/M4A and Ogg metadata parsing, checked against files produced by
//! the system FFmpeg and against ffprobe's own readings. Tests that need
//! FFmpeg skip themselves (with a note) when it is not installed.

use sonora_engine::decode::wav::write_wav;
use sonora_engine::decode::ToolPaths;
use sonora_engine::meta::mp4::{self, parse_audio_specific_config};
use sonora_engine::meta::{ogg, probe_file};
use sonora_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

fn have(tool: &str) -> bool {
    Command::new(tool).arg("-version").output().map(|o| o.status.success()).unwrap_or(false)
}

fn tools_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        let ok = have("ffmpeg") && have("ffprobe");
        if !ok {
            eprintln!("note: ffmpeg/ffprobe not found; skipping FFmpeg-based metadata tests");
        }
        ok
    })
}

/// Whether this FFmpeg can encode with `name` (Homebrew's FFmpeg, for one,
/// is built without libvorbis).
fn has_encoder(name: &str) -> bool {
    Command::new("ffmpeg")
        .args(["-hide_banner", "-encoders"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().any(|l| l.split_whitespace().nth(1) == Some(name)))
        .unwrap_or(false)
}

/// Whether the Ogg Vorbis cases can run.
fn vorbis_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        let ok = tools_available() && has_encoder("libvorbis");
        if tools_available() && !ok {
            eprintln!("note: this FFmpeg has no libvorbis encoder; skipping the Ogg Vorbis cases");
        }
        ok
    })
}

fn workdir() -> PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let d = std::env::temp_dir().join(format!("sonora-meta-formats-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    })
    .clone()
}

/// 3 s of a deterministic, broadband, music-like stereo signal (24-bit WAV).
fn source_wav() -> PathBuf {
    static SRC: OnceLock<PathBuf> = OnceLock::new();
    SRC.get_or_init(|| {
        let path = workdir().join("source.wav");
        let rate = 44_100usize;
        let mut seed = 0x1234_5678u32;
        let mut noise = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed as f32 / u32::MAX as f32) * 2.0 - 1.0
        };
        let mut samples = Vec::with_capacity(rate * 3 * 2);
        for i in 0..rate * 3 {
            let t = i as f32 / rate as f32;
            let tone = 0.3 * (2.0 * std::f32::consts::PI * 110.0 * t).sin() + 0.15 * (2.0 * std::f32::consts::PI * 1760.0 * t).sin();
            let hat = if (i % 11025) < 900 { 0.12 * noise() } else { 0.01 * noise() };
            samples.push(tone + hat);
            samples.push(0.9 * tone - hat);
        }
        write_wav(&path, rate as u32, 2, 24, false, &samples).unwrap();
        path
    })
    .clone()
}

fn ffmpeg(args: &[&str]) {
    let out = Command::new("ffmpeg")
        .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y"])
        .args(args)
        .output()
        .expect("run ffmpeg");
    assert!(out.status.success(), "ffmpeg {:?} failed: {}", args, String::from_utf8_lossy(&out.stderr));
}

/// Encodes the source with extra ffmpeg arguments into `name`.
fn encode(name: &str, args: &[&str]) -> PathBuf {
    let out = workdir().join(name);
    let src = source_wav();
    let mut full: Vec<&str> = vec!["-i", src.to_str().unwrap()];
    full.extend_from_slice(args);
    full.push(out.to_str().unwrap());
    ffmpeg(&full);
    out
}

const TAGS: [&str; 6] = ["-metadata", "title=Night Drive", "-metadata", "artist=Somebody & Co", "-metadata", "album=Late Tapes"];

struct Probe {
    duration: f64,
    sample_rate: u32,
    channels: u16,
    bit_rate: Option<f64>,
    format_bit_rate: Option<f64>,
    bits_per_raw_sample: Option<u16>,
}

fn num(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::String(s) => s.parse().ok(),
        other => other.as_f64(),
    }
}

fn ffprobe(path: &Path) -> Probe {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams"])
        .arg(path)
        .output()
        .expect("run ffprobe");
    let v = sonora_json::parse(&String::from_utf8_lossy(&out.stdout)).expect("ffprobe json");
    let streams = v.get("streams").and_then(|s| s.as_array()).cloned().unwrap_or_default();
    let audio = streams.iter().find(|s| s.get("codec_type").and_then(|t| t.as_str()) == Some("audio")).expect("audio stream").clone();
    let duration = num(audio.get("duration")).or_else(|| num(v.get("format").and_then(|f| f.get("duration")))).expect("duration");
    Probe {
        duration,
        sample_rate: num(audio.get("sample_rate")).unwrap() as u32,
        channels: num(audio.get("channels")).unwrap() as u16,
        bit_rate: num(audio.get("bit_rate")),
        format_bit_rate: num(v.get("format").and_then(|f| f.get("bit_rate"))),
        bits_per_raw_sample: num(audio.get("bits_per_raw_sample")).map(|b| b as u16),
    }
}

fn tag<'a>(tags: &'a [(String, String)], key: &str) -> Option<&'a str> {
    tags.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

fn assert_close(a: f64, b: f64, tol: f64, what: &str) {
    assert!((a - b).abs() <= tol, "{what}: {a} vs {b} (tolerance {tol})");
}

// ---------------------------------------------------------------- MP4 / M4A

#[test]
fn m4a_aac_lc_128_and_256() {
    if !tools_available() {
        return;
    }
    for kbps in ["128k", "256k"] {
        let mut args = vec!["-c:a", "aac", "-b:a", kbps, "-metadata", "tmpo=128"];
        args.extend_from_slice(&TAGS);
        let path = encode(&format!("aac-{kbps}.m4a"), &args);
        let p = ffprobe(&path);
        let m = mp4::parse_file(&path).unwrap();
        assert_eq!(m.codec_id, "aac");
        assert_eq!(m.codec_profile.as_deref(), Some("LC"));
        assert_eq!(m.sample_rate, p.sample_rate);
        assert_eq!(m.channels, p.channels);
        assert_eq!(m.brand.as_deref(), Some("M4A "));
        assert!(!m.has_video && !m.fragmented);
        assert_close(m.duration_secs.unwrap(), p.duration, 0.05, "duration");
        let br = m.avg_bitrate_kbps.unwrap();
        assert_close(br, p.bit_rate.unwrap() / 1000.0, p.bit_rate.unwrap() / 1000.0 * 0.05, "bitrate");
        assert!(m.max_bitrate_kbps.is_some());
        assert!(m.encoder.as_deref().unwrap_or("").starts_with("Lavf"), "encoder {:?}", m.encoder);
        assert_eq!(tag(&m.tags, "title"), Some("Night Drive"));
        assert_eq!(tag(&m.tags, "artist"), Some("Somebody & Co"));
        assert_eq!(tag(&m.tags, "album"), Some("Late Tapes"));
        assert_eq!(tag(&m.tags, "bpm"), Some("128"));

        let facts = probe_file(&path, &ToolPaths::none()).unwrap();
        assert_eq!(facts.codec_id, "aac");
        assert_eq!(facts.codec_name, "AAC-LC");
        assert_eq!(facts.container_name, "M4A");
        assert_eq!(facts.sample_rate, 44_100);
        assert_eq!(facts.channels, 2);
        assert_eq!(facts.lossless, Some(false));
        assert_eq!(facts.bit_depth, None);
        assert_eq!(facts.tags.title.as_deref(), Some("Night Drive"));
        assert_eq!(facts.tags.artist.as_deref(), Some("Somebody & Co"));
        assert_eq!(facts.tags.album.as_deref(), Some("Late Tapes"));
        assert_eq!(facts.tags.bpm, Some(128.0));
        assert!(facts.extension_mismatch.is_none());
        assert_eq!(facts.metadata_source, "native");
    }
}

#[test]
fn m4a_alac_16_and_24_bit() {
    if !tools_available() {
        return;
    }
    for (bits, fmt) in [(16u16, "s16p"), (24, "s32p")] {
        let path = encode(&format!("alac-{bits}.m4a"), &["-c:a", "alac", "-sample_fmt", fmt]);
        let p = ffprobe(&path);
        let m = mp4::parse_file(&path).unwrap();
        assert_eq!(m.codec_id, "alac");
        assert_eq!(m.bits_per_sample, Some(bits));
        assert_eq!(p.bits_per_raw_sample, Some(bits));
        assert_eq!(m.sample_rate, p.sample_rate);
        assert_eq!(m.channels, p.channels);
        assert_close(m.duration_secs.unwrap(), p.duration, 0.05, "duration");
        // The ALAC cookie's bitrate is the uncompressed rate; the measured
        // payload rate must be used instead.
        let br = m.avg_bitrate_kbps.unwrap();
        assert_close(br, p.bit_rate.unwrap() / 1000.0, p.bit_rate.unwrap() / 1000.0 * 0.02, "bitrate");

        let facts = probe_file(&path, &ToolPaths::none()).unwrap();
        assert_eq!(facts.codec_id, "alac");
        assert_eq!(facts.codec_name, "ALAC (Apple Lossless)");
        assert_eq!(facts.lossless, Some(true));
        assert_eq!(facts.bit_depth, Some(bits));
    }
}

#[test]
fn mp4_with_video_selects_the_audio_track() {
    if !tools_available() {
        return;
    }
    let out = workdir().join("video.mp4");
    let src = source_wav();
    ffmpeg(&[
        "-f", "lavfi", "-i", "color=c=black:s=64x64:d=3", "-i", src.to_str().unwrap(),
        "-map", "0:v", "-map", "1:a", "-c:v", "mpeg4", "-c:a", "aac", "-b:a", "160k", "-shortest",
        out.to_str().unwrap(),
    ]);
    let p = ffprobe(&out);
    let m = mp4::parse_file(&out).unwrap();
    assert!(m.has_video);
    assert_eq!(m.codec_id, "aac");
    assert_eq!(m.codec_profile.as_deref(), Some("LC"));
    assert_eq!(m.sample_rate, p.sample_rate);
    assert_eq!(m.channels, p.channels);
    assert_close(m.duration_secs.unwrap(), p.duration, 0.05, "duration");
    let facts = probe_file(&out, &ToolPaths::none()).unwrap();
    assert_eq!(facts.container_name, "MP4");
    assert_eq!(facts.codec_name, "AAC-LC");
}

#[test]
fn faststart_and_default_layouts_agree() {
    if !tools_available() {
        return;
    }
    let mut a = vec!["-c:a", "aac", "-b:a", "192k"];
    a.extend_from_slice(&TAGS);
    let mut b = a.clone();
    b.extend_from_slice(&["-movflags", "+faststart"]);
    let default = encode("layout-default.m4a", &a);
    let fast = encode("layout-faststart.m4a", &b);
    // Confirm the layouts really differ: moov before mdat only with faststart.
    let pos = |p: &Path, k: &[u8]| std::fs::read(p).unwrap().windows(4).position(|w| w == k).unwrap();
    assert!(pos(&fast, b"moov") < pos(&fast, b"mdat"));
    assert!(pos(&default, b"moov") > pos(&default, b"mdat"));
    let x = mp4::parse_file(&default).unwrap();
    let y = mp4::parse_file(&fast).unwrap();
    assert_eq!(x.codec_id, y.codec_id);
    assert_eq!(x.codec_profile, y.codec_profile);
    assert_eq!(x.sample_rate, y.sample_rate);
    assert_eq!(x.channels, y.channels);
    assert_close(x.duration_secs.unwrap(), y.duration_secs.unwrap(), 1e-9, "duration");
    assert_eq!(x.tags, y.tags);
}

// ---------------------------------------------------------------- Ogg

#[test]
fn ogg_vorbis() {
    if !vorbis_available() {
        return;
    }
    let mut args = vec!["-c:a", "libvorbis", "-q:a", "5", "-metadata", "BPM=128"];
    args.extend_from_slice(&TAGS);
    let path = encode("vorbis.ogg", &args);
    let p = ffprobe(&path);
    let o = ogg::parse_file(&path).unwrap();
    assert_eq!(o.codec_id, "vorbis");
    assert_eq!(o.sample_rate, p.sample_rate);
    assert_eq!(o.channels, p.channels);
    assert!(o.nominal_bitrate_kbps.unwrap() > 64.0);
    assert!(o.avg_bitrate_kbps.unwrap() > 32.0);
    assert_close(o.duration_secs.unwrap(), p.duration, 0.05, "duration");
    assert!(o.vendor.is_some());
    assert!(!o.chained);
    assert_eq!(tag(&o.tags, "title"), Some("Night Drive"));
    assert_eq!(tag(&o.tags, "artist"), Some("Somebody & Co"));
    assert_eq!(tag(&o.tags, "album"), Some("Late Tapes"));
    assert_eq!(tag(&o.tags, "bpm"), Some("128"));

    let facts = probe_file(&path, &ToolPaths::none()).unwrap();
    assert_eq!(facts.codec_id, "vorbis");
    assert_eq!(facts.codec_name, "Vorbis");
    assert_eq!(facts.container_name, "Ogg Vorbis");
    assert_eq!(facts.lossless, Some(false));
    assert_eq!(facts.tags.bpm, Some(128.0));
    assert_eq!(facts.tags.title.as_deref(), Some("Night Drive"));
}

#[test]
fn ogg_opus() {
    if !tools_available() {
        return;
    }
    let mut args = vec!["-c:a", "libopus", "-b:a", "128k", "-metadata", "BPM=128"];
    args.extend_from_slice(&TAGS);
    let path = encode("opus.opus", &args);
    let p = ffprobe(&path);
    let o = ogg::parse_file(&path).unwrap();
    assert_eq!(o.codec_id, "opus");
    assert_eq!(o.sample_rate, 48_000);
    assert_eq!(p.sample_rate, 48_000);
    // FFmpeg resamples to 48 kHz before libopus, so that is the recorded input rate.
    assert_eq!(o.input_sample_rate, Some(48_000));
    assert_eq!(o.channels, p.channels);
    // ffprobe does not subtract the pre-skip; ours is the true 3.000 s.
    assert_close(o.duration_secs.unwrap(), p.duration, 0.05, "duration");
    assert_close(o.duration_secs.unwrap(), 3.0, 0.001, "duration minus pre-skip");
    // Opus is VBR: compare with ffprobe's file-level rate (size / duration),
    // which also counts the headers.
    let file_kbps = p.format_bit_rate.unwrap() / 1000.0;
    assert_close(o.avg_bitrate_kbps.unwrap(), file_kbps, file_kbps * 0.04, "bitrate");
    // B36: a large picture (here a long comment) in the headers is not audio.
    let meta = workdir().join("big-tags.txt");
    std::fs::write(&meta, format!(";FFMETADATA1\nDESCRIPTION={}\n", "x".repeat(300_000))).unwrap();
    let args = ["-i", meta.to_str().unwrap(), "-map", "0:a", "-map_metadata", "1", "-c:a", "libopus", "-b:a", "128k"];
    let fat = ogg::parse_file(&encode("opus-big-tags.opus", &args)).unwrap();
    assert_close(fat.avg_bitrate_kbps.unwrap(), o.avg_bitrate_kbps.unwrap(), o.avg_bitrate_kbps.unwrap() * 0.05, "bitrate with a large comment");
    assert_eq!(tag(&o.tags, "title"), Some("Night Drive"));
    assert_eq!(tag(&o.tags, "bpm"), Some("128"));

    let facts = probe_file(&path, &ToolPaths::none()).unwrap();
    assert_eq!(facts.codec_id, "opus");
    assert_eq!(facts.codec_name, "Opus");
    assert_eq!(facts.container_name, "Ogg Opus");
    assert_eq!(facts.sample_rate, 48_000);
    assert_eq!(facts.channels, 2);
    assert_eq!(facts.tags.artist.as_deref(), Some("Somebody & Co"));
}

#[test]
fn ogg_flac() {
    if !tools_available() {
        return;
    }
    let mut args = vec!["-c:a", "flac", "-metadata", "BPM=128"];
    args.extend_from_slice(&TAGS);
    args.extend_from_slice(&["-f", "ogg"]);
    let path = encode("flac.oga", &args);
    let p = ffprobe(&path);
    let o = ogg::parse_file(&path).unwrap();
    assert_eq!(o.codec_id, "flac");
    assert_eq!(o.sample_rate, p.sample_rate);
    assert_eq!(o.channels, p.channels);
    assert_eq!(o.bits_per_sample, p.bits_per_raw_sample);
    assert_eq!(o.bits_per_sample, Some(24));
    assert_close(o.duration_secs.unwrap(), p.duration, 0.05, "duration");
    assert_eq!(tag(&o.tags, "album"), Some("Late Tapes"));
    assert_eq!(tag(&o.tags, "bpm"), Some("128"));

    let facts = probe_file(&path, &ToolPaths::none()).unwrap();
    assert_eq!(facts.codec_id, "flac");
    assert_eq!(facts.container_name, "Ogg FLAC");
    assert_eq!(facts.lossless, Some(true));
    assert_eq!(facts.bit_depth, Some(24));
    assert_eq!(facts.tags.title.as_deref(), Some("Night Drive"));
}

#[test]
fn ogg_multiplexed_streams_are_flagged() {
    if !vorbis_available() {
        return;
    }
    // Two audio streams in one Ogg file: the first is selected, and the file is flagged.
    let out = workdir().join("two-streams.ogg");
    let src = source_wav();
    ffmpeg(&[
        "-i", src.to_str().unwrap(), "-i", src.to_str().unwrap(),
        "-map", "0:a", "-map", "1:a", "-c:a", "libvorbis", "-q:a", "3", out.to_str().unwrap(),
    ]);
    let o = ogg::parse_file(&out).unwrap();
    assert_eq!(o.codec_id, "vorbis");
    assert!(o.chained);
}

// ---------------------------------------------------------------- AudioSpecificConfig

/// Packs a string of '0'/'1' characters (spaces ignored) into bytes, zero padded.
fn bits(s: &str) -> Vec<u8> {
    let v: Vec<u8> = s.bytes().filter(|c| *c == b'0' || *c == b'1').map(|c| c - b'0').collect();
    v.chunks(8)
        .map(|c| c.iter().enumerate().fold(0u8, |acc, (i, b)| acc | (b << (7 - i))))
        .collect()
}

#[test]
fn asc_aac_lc() {
    let c = parse_audio_specific_config(&bits("00010 0100 0010 000")).unwrap();
    assert_eq!(c.object_type, 2);
    assert_eq!(c.core_sample_rate, 44_100);
    assert_eq!(c.channels, 2);
    assert!(!c.sbr && !c.ps);
    assert_eq!(c.profile(), "LC");
    assert_eq!(c.output_sample_rate(), 44_100);
    // The two-byte config written by FFmpeg for 44.1 kHz stereo LC.
    assert_eq!(parse_audio_specific_config(&[0x12, 0x10]).unwrap(), c);
}

#[test]
fn asc_explicit_he_aac() {
    let c = parse_audio_specific_config(&bits("00101 0110 0010 0011 00010 000")).unwrap();
    assert!(c.sbr && !c.ps);
    assert_eq!(c.object_type, 2);
    assert_eq!(c.core_sample_rate, 24_000);
    assert_eq!(c.extension_sample_rate, Some(48_000));
    assert_eq!(c.output_sample_rate(), 48_000);
    assert_eq!(c.channels, 2);
    assert_eq!(c.profile(), "HE-AAC");
}

#[test]
fn asc_implicit_sbr_and_ps() {
    // LC core at 22.05 kHz, then sync extension 0x2B7 -> SBR at 44.1 kHz.
    let sbr = "00010 0111 0010 000 01010110111 00101 1 0100";
    let c = parse_audio_specific_config(&bits(sbr)).unwrap();
    assert!(c.sbr && !c.ps);
    assert_eq!(c.output_sample_rate(), 44_100);
    assert_eq!(c.profile(), "HE-AAC");
    // Mono core with SBR followed by sync extension 0x548 -> PS (stereo output).
    let ps = "00010 0111 0001 000 01010110111 00101 1 0100 10101001000 1";
    let c = parse_audio_specific_config(&bits(ps)).unwrap();
    assert!(c.sbr && c.ps);
    assert_eq!(c.channels, 2);
    assert_eq!(c.profile(), "HE-AACv2");
}

#[test]
fn asc_explicit_ps() {
    let c = parse_audio_specific_config(&bits("11101 0110 0001 0011 00010 000")).unwrap();
    assert!(c.sbr && c.ps);
    assert_eq!(c.core_sample_rate, 24_000);
    assert_eq!(c.output_sample_rate(), 48_000);
    assert_eq!(c.channels, 2);
    assert_eq!(c.profile(), "HE-AACv2");
}

#[test]
fn asc_escape_object_type_and_explicit_frequency() {
    // AOT 31 escape + 6 bits (10) -> 42 (USAC / xHE-AAC).
    let c = parse_audio_specific_config(&bits("11111 001010 0011 0010")).unwrap();
    assert_eq!(c.object_type, 42);
    assert_eq!(c.core_sample_rate, 48_000);
    assert_eq!(c.profile(), "xHE-AAC");
    // Frequency index 15 followed by an explicit 24-bit rate (44100 = 0x00AC44).
    let c = parse_audio_specific_config(&bits("00010 1111 000000001010110001000100 0010 000")).unwrap();
    assert_eq!(c.core_sample_rate, 44_100);
    assert_eq!(c.profile(), "LC");
    // Reserved frequency index 13 is invalid.
    assert!(parse_audio_specific_config(&bits("00010 1101 0010 000")).is_none());
    assert!(parse_audio_specific_config(&[0x12]).is_none());
}

#[test]
fn asc_program_config_element_channels() {
    // channelConfiguration 0: one front CPE + one back CPE + one LFE = 5 channels.
    let s = "00010 0100 0000 000 \
             0000 01 0100 0001 0000 0001 01 000 0000 0 0 0 \
             1 0000 1 0001 0000 \
             00000000";
    let c = parse_audio_specific_config(&bits(s)).unwrap();
    assert_eq!(c.channel_config, 0);
    assert_eq!(c.channels, 5);
}

// ---------------------------------------------------------------- robustness

fn xorshift_bytes(seed: u32, len: usize) -> Vec<u8> {
    let mut s = seed.max(1);
    (0..len)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            s as u8
        })
        .collect()
}

#[test]
fn random_bytes_never_panic() {
    let dir = workdir();
    for seed in 1..40u32 {
        let len = (seed as usize * 977) % 70_000 + 1;
        let mut mp4_bytes = Vec::new();
        mp4_bytes.extend_from_slice(&20u32.to_be_bytes());
        mp4_bytes.extend_from_slice(b"ftypM4A \0\0\0\0M4A ");
        mp4_bytes.extend_from_slice(&xorshift_bytes(seed, len));
        let p = dir.join(format!("random-{seed}.m4a"));
        std::fs::write(&p, &mp4_bytes).unwrap();
        let _ = mp4::parse_file(&p);
        // A random "moov" with random children must not panic either.
        let mut moov = Vec::new();
        let body = xorshift_bytes(seed.wrapping_mul(7), len.min(4096));
        moov.extend_from_slice(&20u32.to_be_bytes());
        moov.extend_from_slice(b"ftypM4A \0\0\0\0M4A ");
        moov.extend_from_slice(&((body.len() + 8) as u32).to_be_bytes());
        moov.extend_from_slice(b"moov");
        moov.extend_from_slice(&body);
        std::fs::write(&p, &moov).unwrap();
        let _ = mp4::parse_file(&p);

        let mut ogg_bytes = b"OggS\0\x02".to_vec();
        ogg_bytes.extend_from_slice(&xorshift_bytes(seed ^ 0xABCD, len));
        let q = dir.join(format!("random-{seed}.ogg"));
        std::fs::write(&q, &ogg_bytes).unwrap();
        assert!(ogg::parse_file(&q).is_err(), "random Ogg bytes must not parse (seed {seed})");
    }
    let empty = dir.join("empty.m4a");
    std::fs::write(&empty, b"").unwrap();
    assert!(mp4::parse_file(&empty).is_err());
    assert!(ogg::parse_file(&empty).is_err());
    let text = dir.join("text.m4a");
    std::fs::write(&text, b"this is a plain text file, not a movie").unwrap();
    assert!(mp4::parse_file(&text).is_err());
}

#[test]
fn truncated_files_never_panic() {
    if !tools_available() {
        return;
    }
    let dir = workdir();
    let mut files = vec![
        encode("trunc-default.m4a", &["-c:a", "aac", "-b:a", "128k"]),
        encode("trunc-fast.m4a", &["-c:a", "aac", "-b:a", "128k", "-movflags", "+faststart"]),
        encode("trunc.opus", &["-c:a", "libopus", "-b:a", "96k"]),
    ];
    if vorbis_available() {
        files.push(encode("trunc.ogg", &["-c:a", "libvorbis", "-q:a", "3"]));
    }
    for f in &files {
        let bytes = std::fs::read(f).unwrap();
        let n = bytes.len();
        for cut in [0usize, 1, 7, 8, 27, 100, 500, n / 3, n / 2, n - 1] {
            let p = dir.join(format!("cut-{}-{cut}", f.file_name().unwrap().to_string_lossy()));
            std::fs::write(&p, &bytes[..cut.min(n)]).unwrap();
            let is_mp4 = f.extension().map(|e| e == "m4a").unwrap_or(false);
            if is_mp4 {
                let r = mp4::parse_file(&p);
                // Without faststart the movie box sits at the end, so any cut loses it.
                if f.to_string_lossy().contains("default") {
                    assert!(r.is_err(), "cut {cut} of {} should fail", f.display());
                }
            } else {
                let r = ogg::parse_file(&p);
                if cut < 100 {
                    assert!(r.is_err());
                }
            }
        }
    }
    // A faststart file cut in the middle of its media keeps a readable movie box.
    let fast = std::fs::read(&files[1]).unwrap();
    let p = dir.join("fast-half.m4a");
    std::fs::write(&p, &fast[..fast.len() / 2]).unwrap();
    let m = mp4::parse_file(&p).unwrap();
    assert_eq!(m.codec_id, "aac");
}
