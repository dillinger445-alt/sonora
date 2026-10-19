//! DJ-ready exports from real files: native and FFmpeg conversions, MP3
//! re-tagging, tags and artwork, read back with SONORA's own parsers.

use sonora_engine::decode::wav::write_wav;
use sonora_engine::decode::{open_source, DecodeRequest, ToolPaths};
use sonora_engine::export::convert::{convert, ConvertRequest, Method, Quality, Target};
use sonora_engine::export::id3::{Picture, TagSet};
use sonora_engine::export::picture;
use sonora_engine::meta::probe_file;
use sonora_engine::synth::{quantize16, render, SynthOptions};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;

fn dir() -> PathBuf {
    let d = std::env::temp_dir().join(format!("sonora-export-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn tools() -> ToolPaths {
    ToolPaths::discover(&[])
}

fn ffmpeg_or_skip(what: &str) -> Option<PathBuf> {
    let t = tools();
    if t.ffmpeg.is_none() {
        eprintln!("FFmpeg not found: skipping {what}");
    }
    t.ffmpeg
}

fn tags() -> TagSet {
    TagSet {
        title: Some("Night Drive".into()),
        artist: Some("Kõrvits".into()),
        album: Some("Test Pressing".into()),
        genre: Some("Melodic Techno".into()),
        year: Some("2025".into()),
        bpm: Some(124.0),
        key: Some("8A".into()),
        comment: None,
        picture: Picture::from_bytes(vec![0xFF, 0xD8, 0xFF, 0xE0, 9, 9, 9, 9]),
    }
}

fn run(src: &Path, target: Target, quality: Quality, dest: &Path) -> sonora_engine::export::convert::ConvertOutcome {
    let t = tools();
    let facts = probe_file(src, &t).unwrap();
    let cancel = AtomicBool::new(false);
    let tags = tags();
    let req = ConvertRequest { src, facts: &facts, target, quality, tags: &tags, dest, tools: &t, cancel: &cancel };
    let mut last = 0.0;
    let out = convert(&req, &mut |p| {
        assert!(p >= last - 1e-9, "progress goes forward");
        last = p;
    })
    .unwrap_or_else(|e| panic!("{}: {e}", src.display()));
    assert_eq!(last, 1.0);
    out
}

fn decode(path: &Path) -> Vec<f32> {
    let facts = probe_file(path, &tools()).unwrap();
    let mut src = open_source(path, facts.container, &tools(), DecodeRequest::default()).unwrap();
    let (mut all, mut buf) = (Vec::new(), Vec::new());
    while src.read(&mut buf, 8192).unwrap() > 0 {
        all.extend_from_slice(&buf);
    }
    all
}

fn check_tags(path: &Path) {
    let f = probe_file(path, &ToolPaths::none()).unwrap();
    assert_eq!(f.tags.title.as_deref(), Some("Night Drive"), "{}", path.display());
    assert_eq!(f.tags.artist.as_deref(), Some("Kõrvits"));
    assert_eq!(f.tags.genre.as_deref(), Some("Melodic Techno"));
    assert_eq!(f.tags.bpm, Some(124.0));
    assert_eq!(f.tags.key.as_deref(), Some("8A"));
    let pic = picture::extract(path, f.container, &ToolPaths::none()).expect("artwork");
    assert_eq!(pic.mime, "image/jpeg");
}

#[test]
fn sixteen_bit_wav_to_aiff_is_bit_exact() {
    let src = dir().join("src16.wav");
    let audio = quantize16(&render(&SynthOptions { seconds: 6.0, ..Default::default() }));
    write_wav(&src, 44_100, 2, 16, false, &audio).unwrap();
    let dest = dir().join("out16.aiff");
    let out = run(&src, Target::Aiff, Quality::Universal, &dest);
    assert_eq!((out.method, out.bits, out.sample_rate, out.dithered), (Method::Native, Some(16), 44_100, false));
    assert_eq!(decode(&dest), audio, "the samples are unchanged");
    check_tags(&dest);
}

#[test]
fn twenty_four_bit_source_is_dithered_to_sixteen_or_kept() {
    let src = dir().join("src24.wav");
    let audio = render(&SynthOptions { seconds: 5.0, ..Default::default() });
    write_wav(&src, 44_100, 2, 24, false, &audio).unwrap();
    let dest = dir().join("out24to16.wav");
    let out = run(&src, Target::Wav, Quality::Universal, &dest);
    assert_eq!((out.bits, out.dithered), (Some(16), true));
    let back = decode(&dest);
    assert_eq!(back.len(), audio.len());
    let err = back.iter().zip(&audio).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
    assert!(err < 2.0 / 32_768.0, "within dither and rounding: {err}");
    check_tags(&dest);
    let hi = dir().join("out24.aiff");
    let out = run(&src, Target::Aiff, Quality::High, &hi);
    assert_eq!((out.bits, out.dithered, out.sample_rate), (Some(24), false, 44_100));
}

#[test]
fn high_rate_source_is_resampled() {
    let Some(_) = ffmpeg_or_skip("resampling") else { return };
    let src = dir().join("src96.wav");
    let audio = render(&SynthOptions { seconds: 5.0, sample_rate: 96_000, ..Default::default() });
    write_wav(&src, 96_000, 2, 24, false, &audio).unwrap();
    let dest = dir().join("out96.aiff");
    let out = run(&src, Target::Aiff, Quality::Universal, &dest);
    assert_eq!((out.method, out.sample_rate, out.bits), (Method::Converted, 44_100, Some(16)));
    assert!((out.duration - 5.0).abs() < 0.01, "{}", out.duration);
    let hi = dir().join("out96hi.aiff");
    let out = run(&src, Target::Aiff, Quality::High, &hi);
    assert_eq!((out.sample_rate, out.bits), (48_000, Some(24)));
    check_tags(&dest);
}

#[test]
fn mp3_sources_keep_their_frames_and_lossless_ones_are_encoded() {
    let Some(ffmpeg) = ffmpeg_or_skip("MP3 export") else { return };
    if !sonora_engine::export::convert::has_mp3_encoder(&tools()) {
        eprintln!("FFmpeg has no MP3 encoder: skipping");
        return;
    }
    let wav = dir().join("src_for_mp3.wav");
    write_wav(&wav, 44_100, 2, 16, false, &quantize16(&render(&SynthOptions { seconds: 6.0, ..Default::default() }))).unwrap();
    // A tagged MP3 as the source.
    let src = dir().join("src.mp3");
    let ok = Command::new(&ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(&wav)
        .args(["-c:a", "libmp3lame", "-b:a", "192k", "-metadata", "title=Old Title", "-id3v2_version", "3"])
        .arg(&src)
        .status()
        .unwrap()
        .success();
    assert!(ok);
    let copied = dir().join("copied.mp3");
    let out = run(&src, Target::Mp3, Quality::Universal, &copied);
    assert_eq!(out.method, Method::Copied);
    let (a0, a1) = sonora_engine::export::mp3::audio_range(&src).unwrap();
    let (b0, b1) = sonora_engine::export::mp3::audio_range(&copied).unwrap();
    let (old, new) = (std::fs::read(&src).unwrap(), std::fs::read(&copied).unwrap());
    assert_eq!(old[a0 as usize..a1 as usize], new[b0 as usize..b1 as usize], "audio frames untouched");
    check_tags(&copied);
    let f = probe_file(&copied, &tools()).unwrap();
    assert!((f.bitrate_kbps.unwrap() - 192.0).abs() < 2.0);

    let encoded = dir().join("encoded.mp3");
    let out = run(&wav, Target::Mp3, Quality::Universal, &encoded);
    assert_eq!(out.method, Method::Encoded);
    let f = probe_file(&encoded, &tools()).unwrap();
    assert_eq!(f.codec_id, "mp3");
    assert!((f.bitrate_kbps.unwrap() - 320.0).abs() < 2.0, "{:?}", f.bitrate_kbps);
    check_tags(&encoded);
}

#[test]
fn flac_artwork_is_read_and_cancel_stops_early() {
    let Some(ffmpeg) = ffmpeg_or_skip("FLAC artwork") else { return };
    let wav = dir().join("src_for_flac.wav");
    write_wav(&wav, 44_100, 2, 16, false, &quantize16(&render(&SynthOptions { seconds: 4.0, ..Default::default() }))).unwrap();
    let cover = dir().join("cover.png");
    std::fs::write(&cover, b"\x89PNG\r\n\x1a\n0000IHDRnot-a-real-png").unwrap();
    let flac = dir().join("with_cover.flac");
    let status = Command::new(&ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(&wav)
        .arg("-i")
        .arg(&cover)
        .args(["-map", "0:a", "-map", "1:v", "-c:a", "flac", "-c:v", "copy", "-disposition:v", "attached_pic"])
        .arg(&flac)
        .status()
        .unwrap();
    if status.success() {
        let pic = picture::extract(&flac, sonora_engine::decode::Container::Flac, &ToolPaths::none());
        assert_eq!(pic.map(|p| p.mime).as_deref(), Some("image/png"));
    } else {
        eprintln!("this FFmpeg could not attach a picture: skipping the artwork check");
    }
    let t = tools();
    let facts = probe_file(&wav, &t).unwrap();
    let cancel = AtomicBool::new(true);
    let tags = TagSet::default();
    let dest = dir().join("cancelled.aiff");
    let req = ConvertRequest { src: &wav, facts: &facts, target: Target::Aiff, quality: Quality::Universal, tags: &tags, dest: &dest, tools: &t, cancel: &cancel };
    assert!(matches!(convert(&req, &mut |_| {}), Err(sonora_engine::AnalysisError::Cancelled)));
}
