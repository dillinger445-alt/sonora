//! Cross-checks the native FLAC decoder against FFmpeg on real encoder output.
//!
//! Tests that need FFmpeg print a note and pass when it is not on `PATH`
//! (set `SONORA_FFMPEG` to point at a specific binary).

use sonora_engine::decode::flac::{self, FlacSource, OpenError};
use sonora_engine::decode::wav::write_wav;
use sonora_engine::decode::{open_source, ChecksumStatus, Container, DecodeRequest, DecodeStats, PcmSource, StreamInfo, ToolPaths};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

fn ffmpeg() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("SONORA_FFMPEG") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(format!("ffmpeg{}", std::env::consts::EXE_SUFFIX)))
        .find(|p| p.is_file())
}

macro_rules! require_ffmpeg {
    () => {
        match ffmpeg() {
            Some(f) => f,
            None => {
                eprintln!("skipping: ffmpeg not found on PATH");
                return;
            }
        }
    };
}

fn workdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("sonora-flac-it-{}-{}", name, std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Music-like interleaved test signal: a kick pattern, noisy hats, a chord
/// pad and short stretches of digital silence (which encoders code as
/// CONSTANT subframes).
fn synth(rate: u32, ch: usize, secs: f64, seed: u64) -> Vec<f32> {
    let n = (rate as f64 * secs) as usize;
    let mut out = Vec::with_capacity(n * ch);
    let mut st = seed;
    let mut noise = move || {
        st = st.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        ((st >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
    };
    let tau = std::f32::consts::TAU;
    for i in 0..n {
        let t = i as f32 / rate as f32;
        let beat = (t * 2.0).fract();
        let env = (-beat * 18.0).exp();
        let kick = (tau * (50.0 + 80.0 * env) * t).sin() * env;
        let hat = noise() * (-(t * 8.0).fract() * 60.0).exp() * 0.25;
        let pad = 0.2 * ((tau * 220.0 * t).sin() + (tau * 277.18 * t).sin() + (tau * 329.63 * t).sin()) / 3.0;
        let gap = if (1.0..1.1).contains(&(t % 2.0)) { 0.0 } else { 1.0 };
        for c in 0..ch {
            let pan = if c % 2 == 0 { 1.0 } else { 0.8 };
            let v = (kick * 0.7 + hat * pan + pad * (1.0 + 0.1 * c as f32)) * gap;
            out.push(v.clamp(-0.99, 0.99));
        }
    }
    out
}

fn encode(ff: &Path, wav: &Path, out: &Path, args: &[&str]) {
    let status = Command::new(ff)
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(wav)
        .args(["-c:a", "flac"])
        .args(args)
        .args(["-metadata", "title=SONORA Test", "-metadata", "artist=Fixture Artist"])
        .arg(out)
        .status()
        .expect("run ffmpeg");
    assert!(status.success(), "ffmpeg failed to encode {} with {args:?}", out.display());
}

/// FFmpeg's own decode, as left-aligned 32-bit integers.
fn reference_s32(ff: &Path, path: &Path) -> Vec<i32> {
    let out = Command::new(ff)
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(path)
        .args(["-f", "s32le", "-acodec", "pcm_s32le", "-"])
        .output()
        .expect("run ffmpeg");
    assert!(out.status.success(), "ffmpeg failed to decode {}", path.display());
    out.stdout.as_chunks::<4>().0.iter().map(|c| i32::from_le_bytes(*c)).collect()
}

fn decode_native(path: &Path, chunk: usize) -> (Vec<f32>, StreamInfo, DecodeStats) {
    let mut src = match FlacSource::open(path) {
        Ok(s) => s,
        Err(OpenError::NotNative(m)) => panic!("{}: not native: {m}", path.display()),
        Err(OpenError::Failed(e)) => panic!("{}: failed: {e}", path.display()),
    };
    let info = src.info().clone();
    let mut all = Vec::new();
    let mut buf = Vec::new();
    loop {
        let n = src.read(&mut buf, chunk).expect("read");
        if n == 0 {
            break;
        }
        assert_eq!(buf.len(), n * info.channels as usize);
        all.extend_from_slice(&buf);
    }
    let stats = src.finish();
    (all, info, stats)
}

fn left_aligned(x: f32, bps: u32) -> i32 {
    let v = (x as f64 * (1u64 << (bps - 1)) as f64).round() as i64;
    (v << (32 - bps)) as i32
}

struct Case {
    name: &'static str,
    rate: u32,
    channels: usize,
    bits: u16,
    secs: f64,
    args: &'static [&'static str],
}

#[test]
fn bit_exact_against_ffmpeg() {
    let ff = require_ffmpeg!();
    let dir = workdir("exact");
    let s16 = &[] as &[&str];
    let cases = [
        Case { name: "st16_l0", rate: 44_100, channels: 2, bits: 16, secs: 3.0, args: &["-compression_level", "0"] },
        Case { name: "st16_l5", rate: 44_100, channels: 2, bits: 16, secs: 3.0, args: &["-compression_level", "5"] },
        Case { name: "st16_l8", rate: 44_100, channels: 2, bits: 16, secs: 3.0, args: &["-compression_level", "8"] },
        Case { name: "st16_l12", rate: 44_100, channels: 2, bits: 16, secs: 3.0, args: &["-compression_level", "12"] },
        Case {
            name: "st16_cholesky",
            rate: 44_100,
            channels: 2,
            bits: 16,
            secs: 3.0,
            args: &["-compression_level", "8", "-lpc_type", "cholesky", "-lpc_passes", "2"],
        },
        Case { name: "st16_fixed", rate: 44_100, channels: 2, bits: 16, secs: 2.0, args: &["-lpc_type", "fixed"] },
        Case { name: "st16_indep", rate: 44_100, channels: 2, bits: 16, secs: 2.0, args: &["-ch_mode", "0"] },
        Case { name: "st16_left_side", rate: 44_100, channels: 2, bits: 16, secs: 2.0, args: &["-ch_mode", "1"] },
        Case { name: "st16_right_side", rate: 44_100, channels: 2, bits: 16, secs: 2.0, args: &["-ch_mode", "2"] },
        Case { name: "st16_mid_side", rate: 44_100, channels: 2, bits: 16, secs: 2.0, args: &["-ch_mode", "3"] },
        Case { name: "st16_exact_rice", rate: 44_100, channels: 2, bits: 16, secs: 2.0, args: &["-exact_rice_parameters", "1"] },
        Case { name: "st24_l0", rate: 96_000, channels: 2, bits: 24, secs: 2.0, args: &["-sample_fmt", "s32", "-compression_level", "0"] },
        Case { name: "st24_l5", rate: 96_000, channels: 2, bits: 24, secs: 2.0, args: &["-sample_fmt", "s32", "-compression_level", "5"] },
        Case { name: "st24_l8", rate: 96_000, channels: 2, bits: 24, secs: 2.0, args: &["-sample_fmt", "s32", "-compression_level", "8"] },
        Case { name: "st24_l12", rate: 96_000, channels: 2, bits: 24, secs: 2.0, args: &["-sample_fmt", "s32", "-compression_level", "12"] },
        Case { name: "mono16_l0", rate: 48_000, channels: 1, bits: 16, secs: 2.0, args: &["-compression_level", "0"] },
        Case { name: "mono16_l5", rate: 48_000, channels: 1, bits: 16, secs: 2.0, args: &["-compression_level", "5"] },
        Case { name: "mono16_l8", rate: 48_000, channels: 1, bits: 16, secs: 2.0, args: &["-compression_level", "8"] },
        Case { name: "mono16_l12", rate: 48_000, channels: 1, bits: 16, secs: 2.0, args: &["-compression_level", "12"] },
        Case { name: "st16_default", rate: 44_100, channels: 2, bits: 16, secs: 2.0, args: s16 },
    ];
    for (i, case) in cases.iter().enumerate() {
        let wav = dir.join(format!("{}.wav", case.name));
        let flac_path = dir.join(format!("{}.flac", case.name));
        let pcm = synth(case.rate, case.channels, case.secs, 1000 + i as u64);
        write_wav(&wav, case.rate, case.channels as u16, case.bits, false, &pcm).unwrap();
        encode(&ff, &wav, &flac_path, case.args);

        let frames = pcm.len() / case.channels;
        let h = flac::parse_header(&flac_path).unwrap();
        assert_eq!(h.sample_rate, case.rate, "{}", case.name);
        assert_eq!(h.channels as usize, case.channels, "{}", case.name);
        assert_eq!(h.bits_per_sample, case.bits, "{}", case.name);
        assert_eq!(h.total_frames, Some(frames as u64), "{}", case.name);
        assert!(h.md5.is_some(), "{}: encoder stored no MD5", case.name);
        assert!(h.vendor.is_some(), "{}: no vendor string", case.name);
        assert!(h.tags.iter().any(|(k, v)| k == "title" && v == "SONORA Test"), "{}: tags {:?}", case.name, h.tags);
        assert!(h.tags.iter().any(|(k, v)| k == "artist" && v == "Fixture Artist"), "{}", case.name);
        assert!(!h.id3_prefix);

        let reference = reference_s32(&ff, &flac_path);
        assert_eq!(reference.len(), frames * case.channels, "{}: reference length", case.name);
        for chunk in [4096usize, 1001] {
            let (native, info, stats) = decode_native(&flac_path, chunk);
            assert_eq!(info.decoder, "native-flac");
            assert_eq!(info.bits, Some(case.bits));
            assert_eq!(info.frames_hint, Some(frames as u64));
            assert_eq!(stats.decode_errors, 0, "{}: {:?}", case.name, stats.messages);
            assert!(!stats.truncated, "{}", case.name);
            assert_eq!(stats.checksum, ChecksumStatus::Verified, "{}", case.name);
            assert_eq!(native.len(), reference.len(), "{}: length", case.name);
            let bps = case.bits as u32;
            if let Some(i) = native.iter().zip(&reference).position(|(a, b)| left_aligned(*a, bps) != *b) {
                panic!("{}: first mismatch at sample {i}: native {} vs ffmpeg {}", case.name, left_aligned(native[i], bps), reference[i]);
            }
            // Effective bit depth bookkeeping: only `bits` significant bits are ever set.
            let mask = stats.int_or_mask.unwrap();
            assert_eq!(mask & ((1u32 << (32 - bps)) - 1), 0, "{}", case.name);
        }
    }
}

#[test]
fn thirty_two_bit_stream() {
    let ff = require_ffmpeg!();
    let dir = workdir("s32");
    let wav = dir.join("s32.wav");
    let flac_path = dir.join("s32.flac");
    let pcm = synth(48_000, 2, 1.5, 7);
    write_wav(&wav, 48_000, 2, 32, false, &pcm).unwrap();
    encode(&ff, &wav, &flac_path, &["-sample_fmt", "s32", "-strict", "experimental", "-compression_level", "5"]);
    let h = flac::parse_header(&flac_path).unwrap();
    assert_eq!(h.bits_per_sample, 32);
    let (native, _, stats) = decode_native(&flac_path, 3000);
    // The MD5 covers the exact 32-bit integers, so a match proves bit-exactness.
    assert_eq!(stats.checksum, ChecksumStatus::Verified);
    assert_eq!(stats.decode_errors, 0);
    let reference = reference_s32(&ff, &flac_path);
    assert_eq!(native.len(), reference.len());
    for (a, b) in native.iter().zip(&reference) {
        assert!((*a as f64 - *b as f64 / 2_147_483_648.0).abs() < 1e-7);
    }
}

#[test]
fn engine_integration_and_id3_prefix() {
    let ff = require_ffmpeg!();
    let dir = workdir("integration");
    let wav = dir.join("in.wav");
    let flac_path = dir.join("in.flac");
    let pcm = synth(44_100, 2, 1.0, 3);
    write_wav(&wav, 44_100, 2, 16, false, &pcm).unwrap();
    encode(&ff, &wav, &flac_path, &["-compression_level", "5"]);

    // open_source picks the native decoder without any external tools.
    let mut src = open_source(&flac_path, Container::Flac, &ToolPaths::none(), DecodeRequest::default()).expect("open");
    assert_eq!(src.info().decoder, "native-flac");
    let mut buf = Vec::new();
    let mut total = 0;
    loop {
        let n = src.read(&mut buf, 8192).unwrap();
        if n == 0 {
            break;
        }
        total += n;
    }
    assert_eq!(total, pcm.len() / 2);
    assert_eq!(src.finish().checksum, ChecksumStatus::Verified);

    // File facts come from the native header parser.
    let facts = sonora_engine::meta::probe_file(&flac_path, &ToolPaths::none()).expect("probe");
    assert_eq!(facts.codec_id, "flac");
    assert_eq!(facts.sample_rate, 44_100);
    assert_eq!(facts.bit_depth, Some(16));
    assert_eq!(facts.lossless, Some(true));
    assert_eq!(facts.tags.title.as_deref(), Some("SONORA Test"));
    assert!(facts.header_duration_secs.unwrap() > 0.99);

    // A (non-standard) ID3v2 tag in front of "fLaC" is skipped.
    let mut tagged = vec![b'I', b'D', b'3', 4, 0, 0, 0, 0, 0, 30];
    tagged.extend_from_slice(&[0u8; 30]);
    tagged.extend_from_slice(&std::fs::read(&flac_path).unwrap());
    let tagged_path = dir.join("id3.flac");
    std::fs::write(&tagged_path, &tagged).unwrap();
    let h = flac::parse_header(&tagged_path).unwrap();
    assert!(h.id3_prefix);
    let (out, _, stats) = decode_native(&tagged_path, 4096);
    assert_eq!(out.len(), pcm.len());
    assert_eq!(stats.checksum, ChecksumStatus::Verified);
}

#[test]
fn corrupted_and_truncated_real_files() {
    let ff = require_ffmpeg!();
    let dir = workdir("damage");
    let wav = dir.join("in.wav");
    let flac_path = dir.join("in.flac");
    let pcm = synth(44_100, 2, 4.0, 11);
    let frames = pcm.len() / 2;
    write_wav(&wav, 44_100, 2, 16, false, &pcm).unwrap();
    encode(&ff, &wav, &flac_path, &["-compression_level", "5"]);
    let clean = std::fs::read(&flac_path).unwrap();

    // Flipped bytes inside two audio frames.
    let mut damaged = clean.clone();
    for at in [clean.len() * 2 / 5, clean.len() * 3 / 5] {
        for b in &mut damaged[at..at + 32] {
            *b ^= 0xA5;
        }
    }
    let damaged_path = dir.join("damaged.flac");
    std::fs::write(&damaged_path, &damaged).unwrap();
    let (out, _, stats) = decode_native(&damaged_path, 4096);
    assert!(stats.decode_errors >= 2, "errors {} {:?}", stats.decode_errors, stats.messages);
    assert_eq!(stats.checksum, ChecksumStatus::Mismatch);
    assert!(!stats.messages.is_empty());
    assert!(out.len() / 2 >= frames * 9 / 10, "only {} of {frames} frames decoded", out.len() / 2);

    // Truncated download.
    let cut_path = dir.join("cut.flac");
    std::fs::write(&cut_path, &clean[..clean.len() * 7 / 10]).unwrap();
    let (out, _, stats) = decode_native(&cut_path, 4096);
    assert!(stats.truncated);
    assert_eq!(stats.checksum, ChecksumStatus::Mismatch);
    assert!(out.len() / 2 < frames);
    assert!(out.len() / 2 > frames / 2);
}

#[test]
fn random_damage_never_panics() {
    let ff = require_ffmpeg!();
    let dir = workdir("fuzz");
    let wav = dir.join("in.wav");
    let flac_path = dir.join("in.flac");
    let pcm = synth(44_100, 2, 0.5, 5);
    write_wav(&wav, 44_100, 2, 16, false, &pcm).unwrap();
    encode(&ff, &wav, &flac_path, &["-compression_level", "8"]);
    let clean = std::fs::read(&flac_path).unwrap();
    let case_path = dir.join("case.flac");
    let mut st = 0xDEAD_BEEF_u64;
    let mut rnd = move |n: usize| {
        st = st.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        ((st >> 33) as usize) % n.max(1)
    };
    for round in 0..200 {
        let mut bytes = clean.clone();
        match round % 5 {
            0 => {
                for _ in 0..1 + rnd(16) {
                    let i = rnd(bytes.len());
                    bytes[i] ^= 1 << rnd(8);
                }
            }
            1 => bytes.truncate(rnd(bytes.len())),
            2 => {
                let i = rnd(bytes.len());
                let n = rnd(512).min(bytes.len() - i);
                for b in &mut bytes[i..i + n] {
                    *b = rnd(256) as u8;
                }
            }
            3 => {
                // Duplicate a chunk (spliced streams).
                let i = rnd(bytes.len());
                let n = rnd(4096).min(bytes.len() - i);
                let chunk = bytes[i..i + n].to_vec();
                let at = rnd(bytes.len());
                bytes.splice(at..at, chunk);
            }
            _ => {
                let i = rnd(bytes.len());
                bytes[i] = 0xFF;
                if i + 1 < bytes.len() {
                    bytes[i + 1] = 0xF8;
                }
            }
        }
        std::fs::write(&case_path, &bytes).unwrap();
        let _ = flac::parse_header(&case_path);
        if let Ok(mut src) = FlacSource::open(&case_path) {
            let mut buf = Vec::new();
            let mut total = 0usize;
            while let Ok(n) = src.read(&mut buf, 2048) {
                if n == 0 {
                    break;
                }
                total += n;
                assert!(total < 50 * pcm.len(), "runaway output in round {round}");
            }
            let _ = src.finish();
        }
    }
}

#[test]
fn rejects_non_flac() {
    let dir = workdir("junk");
    let p = dir.join("junk.flac");
    std::fs::write(&p, b"definitely not a FLAC stream, only some text pretending to be one").unwrap();
    assert!(flac::parse_header(&p).is_err());
    assert!(matches!(FlacSource::open(&p), Err(OpenError::Failed(_))));
    let empty = dir.join("empty.flac");
    std::fs::write(&empty, b"").unwrap();
    assert!(flac::parse_header(&empty).is_err());
    assert!(matches!(FlacSource::open(&empty), Err(OpenError::Failed(_))));
    let only_magic = dir.join("magic.flac");
    std::fs::write(&only_magic, b"fLaC\x00\x00\x00\x22").unwrap();
    assert!(matches!(FlacSource::open(&only_magic), Err(OpenError::Failed(_))));
}

#[test]
fn decode_speed() {
    let ff = require_ffmpeg!();
    let dir = workdir("speed");
    let wav = dir.join("in.wav");
    let flac_path = dir.join("in.flac");
    let secs = 30.0;
    let pcm = synth(44_100, 2, secs, 21);
    write_wav(&wav, 44_100, 2, 16, false, &pcm).unwrap();
    encode(&ff, &wav, &flac_path, &["-compression_level", "5"]);
    let t0 = Instant::now();
    let (out, _, stats) = decode_native(&flac_path, 4096);
    let elapsed = t0.elapsed().as_secs_f64();
    assert_eq!(out.len(), pcm.len());
    assert_eq!(stats.checksum, ChecksumStatus::Verified);
    let factor = secs / elapsed;
    eprintln!("native FLAC decode (incl. MD5): {secs:.0} s of 44.1 kHz/16-bit stereo in {:.1} ms = {factor:.0}x realtime", elapsed * 1000.0);
    assert!(factor > 20.0, "decoder is unexpectedly slow: {factor:.0}x realtime");
}
