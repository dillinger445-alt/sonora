//! End-to-end analysis tests on files with known signal conditions.
//!
//! Lossless cases are written as WAV and decoded natively (no external
//! tools). Lossy cases and the loudness cross-check use the system FFmpeg and
//! are skipped with a note when it is not installed.

use sonora_engine::analysis::measure::Measurements;
use sonora_engine::analysis::Control;
use sonora_engine::assess::{assess, Evidence, ScoringConfig, Severity};
use sonora_engine::decode::wav::write_wav;
use sonora_engine::decode::ToolPaths;
use sonora_engine::dsp::biquad::{BiquadCoef, BiquadState};
use sonora_engine::synth::{quantize16, render, white_noise, SynthOptions};
use sonora_engine::{analyze_file, AnalyzeOptions, FileAnalysis};
use sonora_json::ToJson;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;

const FS: u32 = 44_100;

fn dir() -> PathBuf {
    let d = std::env::temp_dir().join(format!("sonora-signals-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn track(seconds: f64) -> Vec<f32> {
    render(&SynthOptions { seconds, ..Default::default() })
}

fn write(name: &str, bits: u16, samples: &[f32]) -> PathBuf {
    let p = dir().join(name);
    write_wav(&p, FS, 2, bits, false, samples).unwrap();
    p
}

fn run(path: &Path, tools: ToolPaths) -> FileAnalysis {
    let cancel = AtomicBool::new(false);
    let mut progress = |_f: f64| {};
    let mut ctl = Control { cancel: &cancel, progress: &mut progress, max_duration_secs: 3600.0 };
    let opts = AnalyzeOptions { tools, hash_file: true, ..Default::default() };
    analyze_file(path, &opts, &mut ctl).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn native(path: &Path) -> FileAnalysis {
    run(path, ToolPaths::none())
}

fn ids(a: &FileAnalysis) -> Vec<String> {
    a.assessment.findings.iter().map(|f| f.id.clone()).collect()
}

fn find<'a>(a: &'a FileAnalysis, id: &str) -> Option<&'a sonora_engine::assess::Finding> {
    a.assessment.findings.iter().find(|f| f.id == id)
}

fn ffmpeg() -> Option<PathBuf> {
    let t = ToolPaths::discover(&[]);
    if t.ffmpeg.is_none() {
        eprintln!("note: FFmpeg not found; skipping FFmpeg-based checks");
    }
    t.ffmpeg
}

fn ff(ffmpeg: &Path, args: &[&str]) {
    let st = Command::new(ffmpeg).args(["-hide_banner", "-loglevel", "error", "-y"]).args(args).status().unwrap();
    assert!(st.success(), "ffmpeg {:?}", args);
}

fn lowpass_cascade(x: &[f32], hz: f64, sections: usize) -> Vec<f32> {
    let c = BiquadCoef::lowpass(FS as f64, hz, std::f64::consts::FRAC_1_SQRT_2);
    let mut l = vec![BiquadState::default(); sections];
    let mut r = vec![BiquadState::default(); sections];
    x.as_chunks::<2>()
        .0
        .iter()
        .flat_map(|fr| {
            let (mut a, mut b) = (fr[0] as f64, fr[1] as f64);
            for s in l.iter_mut() {
                a = s.process(&c, a);
            }
            for s in r.iter_mut() {
                b = s.process(&c, b);
            }
            [a as f32, b as f32]
        })
        .collect()
}

// ------------------------------------------------------------ clean audio

#[test]
fn clean_master_scores_high_with_no_deductions() {
    let a = native(&write("clean.wav", 16, &quantize16(&track(40.0))));
    let s = &a.assessment;
    assert!(s.scores.overall >= 98.0 && s.scores.dj >= 98.0 && s.scores.listening >= 98.0, "{:?} {:?}", s.scores, ids(&a));
    assert!(s.findings.iter().all(|f| f.effective_penalty().overall == 0.0), "{:?}", ids(&a));
    assert!(s.verdict.starts_with("Clean"), "{}", s.verdict);
    assert_eq!(s.confidence_label, "High");
    let m = &a.signal.measurements;
    assert!(m.bandwidth.upper_hz > 21_000.0);
    assert!(m.bandwidth.edge.as_ref().is_none_or(|e| !e.is_steep() || e.hz > 20_500.0));
    assert_eq!(m.effective_bits, Some(16));
    assert_eq!(m.clipping.as_ref().unwrap().events, 0);
    assert!(a.file_hash.as_ref().unwrap().len() == 32 && a.signal.audio_hash.len() == 32);
}

// --------------------------------------------------------------- clipping

#[test]
fn hard_clipping_is_detected_and_penalised() {
    let x = track(40.0);
    let clean = native(&write("clip_ref.wav", 16, &quantize16(&x)));
    let g = 10f32.powf(5.0 / 20.0);
    let clipped: Vec<f32> = x.iter().map(|v| (v * g).clamp(-1.0, 1.0)).collect();
    let a = native(&write("clipped.wav", 16, &quantize16(&clipped)));
    let c = a.signal.measurements.clipping.as_ref().unwrap();
    assert!(c.events > 1000, "events {}", c.events);
    let f = find(&a, "clipping").expect("clipping finding");
    assert!(f.severity >= Severity::High);
    assert_eq!(f.evidence, Evidence::Fact);
    assert!(a.assessment.scores.overall <= clean.assessment.scores.overall - 25.0);
    assert!(a.assessment.flags.contains(&"clipping".to_string()));
    assert!(!a.signal.visuals.clip_times.is_empty());
}

#[test]
fn clipping_below_full_scale_is_detected() {
    let x = track(30.0);
    let g = 10f32.powf(4.0 / 20.0);
    let norm = 10f32.powf(-1.0 / 20.0);
    let y: Vec<f32> = x.iter().map(|v| (v * g).clamp(-1.0, 1.0) * norm).collect();
    let a = native(&write("clip_norm.wav", 16, &quantize16(&y)));
    let c = a.signal.measurements.clipping.as_ref().unwrap();
    assert!(c.events > 500);
    assert!((c.ceiling_dbfs + 1.0).abs() < 0.05, "ceiling {}", c.ceiling_dbfs);
}

#[test]
fn more_clipping_never_scores_higher() {
    let x = track(30.0);
    let mut last = f64::INFINITY;
    for drive in [0.0f32, 1.5, 3.0, 6.0] {
        let g = 10f32.powf(drive / 20.0);
        let y: Vec<f32> = x.iter().map(|v| (v * g).clamp(-1.0, 1.0)).collect();
        let a = native(&write(&format!("drive{drive}.wav"), 16, &quantize16(&y)));
        let s = a.assessment.scores.overall;
        assert!(s <= last, "drive {drive}: {s} > {last}");
        last = s;
    }
}

#[test]
fn isolated_full_scale_samples_are_minor() {
    let mut x = quantize16(&track(30.0));
    // Three single-sample spikes to full scale.
    for &i in &[200_000usize, 600_000, 1_000_000] {
        x[i] = 32767.0 / 32768.0;
    }
    let a = native(&write("spikes.wav", 16, &x));
    if let Some(f) = find(&a, "clipping") {
        assert!(f.severity <= Severity::Low, "{:?}", f.severity);
    }
    assert!(a.assessment.scores.overall >= 95.0);
}

// ---------------------------------------------------------------- silence

#[test]
fn digital_silence_is_reported_as_silent_file() {
    let a = native(&write("silence.wav", 16, &vec![0f32; FS as usize * 2 * 10]));
    assert!(ids(&a).contains(&"silent_file".to_string()));
    assert_eq!(a.assessment.scores.overall, 0.0);
    assert!(a.assessment.verdict.starts_with("Silent"));
    assert!(a.signal.measurements.integrated_lufs.is_none());
    assert_eq!(a.signal.measurements.effective_bits, None);
}

#[test]
fn leading_silence_dropouts_and_zero_filled_tail() {
    let mut x = vec![0f32; FS as usize * 2 * 3];
    x.extend(quantize16(&track(30.0)));
    let n = x.len();
    // Dropout of 120 ms at ~15 s.
    let s = (15.0 * FS as f64) as usize * 2;
    x[s..s + (0.12 * FS as f64) as usize * 2].iter_mut().for_each(|v| *v = 0.0);
    // Zero-filled final 8 s.
    x[n - FS as usize * 2 * 8..].iter_mut().for_each(|v| *v = 0.0);
    let a = native(&write("gaps.wav", 16, &x));
    let m = &a.signal.measurements;
    assert!((m.silence.leading - 3.0).abs() < 0.05, "leading {}", m.silence.leading);
    assert_eq!(m.silence.dropout_count, 1);
    assert!(m.silence.abrupt_cutoff.is_some());
    let i = ids(&a);
    assert!(i.contains(&"dropouts".to_string()) && i.contains(&"abrupt_ending".to_string()) && i.contains(&"leading_silence".to_string()), "{i:?}");
}

// ---------------------------------------------------------- low-level noise

#[test]
fn audible_hiss_is_flagged_weakly_and_inaudible_noise_is_not() {
    for (level_db, expect) in [(-50.0, true), (-85.0, false)] {
        let amp = 10f64.powf(level_db / 20.0) * 3f64.sqrt();
        let noise = white_noise(9, FS as usize * 2 * 40, amp);
        let music = track(35.0);
        // 5 s of noise alone, then music at -6 dB over the noise.
        let mut x: Vec<f32> = noise[..FS as usize * 2 * 5].to_vec();
        x.extend(music.iter().zip(&noise[FS as usize * 2 * 5..]).map(|(m, n)| m * 0.5 + n));
        let a = native(&write(&format!("hiss{level_db}.wav"), 16, &quantize16(&x)));
        let f = find(&a, "noise_floor");
        assert_eq!(f.is_some(), expect, "level {level_db}: {:?} {:?}", ids(&a), a.signal.measurements.noise);
        if let Some(f) = f {
            assert_eq!(f.evidence, Evidence::Weak);
        }
    }
}

#[test]
fn low_level_noise_only_file_is_measured_sensibly() {
    // White noise at -70 dBFS RMS: K-weighted loudness of white noise lies a
    // few dB above its RMS level (the K curve boosts high frequencies).
    let amp = 10f64.powf(-70.0 / 20.0) * 3f64.sqrt();
    let a = native(&write("noise70.wav", 24, &white_noise(3, FS as usize * 2 * 20, amp)));
    let m = &a.signal.measurements;
    assert!((m.rms_dbfs + 70.0).abs() < 0.5, "rms {}", m.rms_dbfs);
    let i = m.integrated_lufs.unwrap();
    assert!(i > -70.0 && i < -63.0, "integrated {i}");
    assert!(m.bandwidth.upper_hz > 20_000.0);
    assert!(!ids(&a).iter().any(|x| x == "lossy_source_suspected" || x == "clipping"));
}

// ------------------------------------------------------ restricted bandwidth

#[test]
fn deliberate_brickwall_is_measured_but_only_weakly_attributed() {
    let y = lowpass_cascade(&track(40.0), 16_000.0, 16);
    let a = native(&write("brick16.wav", 16, &quantize16(&y)));
    let e = a.signal.measurements.bandwidth.edge.clone().expect("edge");
    assert!(e.is_steep() && (e.hz - 16_000.0).abs() < 400.0, "{e:?}");
    let cut = find(&a, "steep_cutoff").expect("steep cutoff is a measured fact");
    assert_eq!(cut.evidence, Evidence::Fact);
    // Without lossy holes and with energy leaking above the cutoff, the
    // provenance conclusion must not be "strong".
    if let Some(f) = find(&a, "lossy_source_suspected") {
        assert_ne!(f.evidence, Evidence::Strong, "{}", f.detail);
    }
}

#[test]
fn gentle_roll_off_is_not_mistaken_for_a_cutoff() {
    let y = lowpass_cascade(&track(40.0), 7_000.0, 1);
    let a = native(&write("dark.wav", 16, &quantize16(&y)));
    let i = ids(&a);
    assert!(!i.contains(&"steep_cutoff".to_string()) && !i.contains(&"lossy_source_suspected".to_string()), "{i:?}");
    assert!(a.assessment.scores.overall >= 95.0);
}

// ------------------------------------------------- stereo, DC, resolution

#[test]
fn polarity_inversion_hits_the_dj_score_hardest() {
    let mut x = quantize16(&track(30.0));
    x.iter_mut().skip(1).step_by(2).for_each(|v| *v = -*v);
    let a = native(&write("inverted.wav", 16, &x));
    let f = find(&a, "phase_cancellation").expect("phase finding");
    assert!(f.severity >= Severity::High);
    let s = a.assessment.scores;
    assert!(s.dj < s.listening && s.dj <= 60.0, "{s:?}");
}

#[test]
fn dc_offset_padding_and_dual_mono() {
    let x = quantize16(&track(30.0));
    let dc: Vec<f32> = x.iter().map(|v| v * 0.9 + 0.03).collect();
    let a = native(&write("dc.wav", 16, &quantize16(&dc)));
    assert!(find(&a, "dc_offset").is_some());

    let p = native(&write("padded.wav", 24, &x));
    assert_eq!(p.signal.measurements.effective_bits, Some(16));
    assert!(find(&p, "padded_bits").is_some());

    let mono: Vec<f32> = x.as_chunks::<2>().0.iter().flat_map(|f| [f[0], f[0]]).collect();
    let m = native(&write("dualmono.wav", 16, &mono));
    assert!(m.signal.measurements.stereo.as_ref().unwrap().identical);
    assert!(find(&m, "dual_mono").is_some());
    assert!(m.assessment.scores.overall >= 98.0);
}

fn write_at(name: &str, fs: u32, bits: u16, samples: &[f32]) -> PathBuf {
    let p = dir().join(name);
    write_wav(&p, fs, 2, bits, false, samples).unwrap();
    p
}

fn bessel_i0(x: f64) -> f64 {
    let (mut sum, mut term) = (1.0, 1.0);
    for k in 1..64 {
        term *= x * x / 4.0 / (k as f64 * k as f64);
        sum += term;
    }
    sum
}

/// 2× upsampling of interleaved stereo the way a sample-rate converter does
/// it: zero-stuffing, then a 255-tap Kaiser-windowed sinc low-pass just
/// below the source's Nyquist frequency (about 100 dB stopband attenuation).
fn upsample2x(x: &[f32]) -> Vec<f32> {
    const TAPS: usize = 255;
    const HALF: usize = TAPS / 2;
    const BETA: f64 = 10.0;
    // Cut-off in cycles per output sample: 0.235 × 96 kHz ≈ 22.6 kHz.
    let fc = 0.235;
    let h: Vec<f64> = (0..TAPS)
        .map(|n| {
            let t = n as f64 - HALF as f64;
            let sinc = if t == 0.0 { 2.0 * fc } else { (2.0 * std::f64::consts::PI * fc * t).sin() / (std::f64::consts::PI * t) };
            let r = t / HALF as f64;
            sinc * bessel_i0(BETA * (1.0 - r * r).max(0.0).sqrt()) / bessel_i0(BETA)
        })
        .collect();
    let frames = x.len() / 2;
    let mut out = vec![0f32; frames * 4];
    for m in 0..frames * 2 {
        // Only even positions of the zero-stuffed signal carry samples.
        let mut acc = [0f64; 2];
        let mut k = (m + HALF) % 2;
        while k < TAPS && k <= m + HALF {
            let i = (m + HALF - k) / 2;
            if i < frames {
                acc[0] += h[k] * x[2 * i] as f64;
                acc[1] += h[k] * x[2 * i + 1] as f64;
            }
            k += 2;
        }
        out[2 * m] = (2.0 * acc[0]) as f32;
        out[2 * m + 1] = (2.0 * acc[1]) as f32;
    }
    out
}

#[test]
fn upsampled_audio_is_told_apart_from_genuine_high_resolution() {
    // A 48 kHz master converted to 96 kHz: nothing above the old Nyquist.
    let master = render(&SynthOptions { seconds: 20.0, sample_rate: 48_000, ..Default::default() });
    let up = native(&write_at("upsampled96.wav", 96_000, 24, &upsample2x(&master)));
    let m = &up.signal.measurements;
    let rel = m.bandwidth.ultrasonic_rel_db.expect("ultrasonic band measured at 96 kHz");
    assert!(rel <= -85.0, "upsampled: {rel} dB above 26 kHz");
    let f = find(&up, "upsampled").expect("upsampling detected");
    assert_eq!(f.evidence, Evidence::Strong);
    assert!(up.assessment.flags.iter().any(|x| x == "upsampled"), "{:?}", up.assessment.flags);
    let s = up.assessment.scores;
    // A small deduction on the overall score only: the audio itself is fine.
    assert!((93.0..=96.0).contains(&s.overall) && s.dj == 100.0 && s.listening == 100.0, "{s:?} {:?}", ids(&up));

    // The same music rendered natively at 96 kHz keeps its ultrasonic content.
    let hires = render(&SynthOptions { seconds: 20.0, sample_rate: 96_000, ..Default::default() });
    let hr = native(&write_at("hires96.wav", 96_000, 24, &hires));
    let rel = hr.signal.measurements.bandwidth.ultrasonic_rel_db.expect("ultrasonic band measured");
    assert!(rel > -85.0, "genuine 96 kHz: {rel} dB above 26 kHz");
    assert!(find(&hr, "upsampled").is_none(), "{:?}", ids(&hr));
    assert!(hr.assessment.scores.overall >= 98.0, "{:?} {:?}", hr.assessment.scores, ids(&hr));
}

#[test]
fn truncated_wav_is_reported() {
    let p = write("trunc.wav", 16, &quantize16(&track(30.0)));
    let bytes = std::fs::read(&p).unwrap();
    std::fs::write(&p, &bytes[..bytes.len() / 2]).unwrap();
    let a = native(&p);
    assert!(a.signal.measurements.truncated);
    let f = find(&a, "truncated").expect("truncated finding");
    assert_eq!(f.severity, Severity::Critical);
    assert!(a.assessment.scores.overall <= 60.0);
}

#[test]
fn broken_files_fail_gracefully() {
    let d = dir();
    std::fs::write(d.join("text.wav"), b"definitely not audio").unwrap();
    std::fs::write(d.join("empty.flac"), b"").unwrap();
    let opts = AnalyzeOptions { tools: ToolPaths::none(), ..Default::default() };
    for name in ["text.wav", "empty.flac", "missing.mp3"] {
        let cancel = AtomicBool::new(false);
        let mut progress = |_f: f64| {};
        let mut ctl = Control { cancel: &cancel, progress: &mut progress, max_duration_secs: 3600.0 };
        let r = analyze_file(&d.join(name), &opts, &mut ctl);
        assert!(r.is_err(), "{name} should fail");
    }
}

#[test]
fn cancellation_stops_analysis() {
    let p = write("cancel.wav", 16, &quantize16(&track(30.0)));
    let cancel = AtomicBool::new(true);
    let mut progress = |_f: f64| {};
    let mut ctl = Control { cancel: &cancel, progress: &mut progress, max_duration_secs: 3600.0 };
    let r = analyze_file(&p, &AnalyzeOptions { tools: ToolPaths::none(), hash_file: false, ..Default::default() }, &mut ctl);
    assert!(matches!(r, Err(sonora_engine::AnalysisError::Cancelled)));
}

// --------------------------------------------- determinism & re-scoring

#[test]
fn analysis_is_deterministic_and_rescorable_from_stored_measurements() {
    let p = write("determinism.wav", 16, &quantize16(&track(25.0)));
    let a = native(&p);
    let b = native(&p);
    assert_eq!(a.assessment.to_json().to_string_compact(), b.assessment.to_json().to_string_compact());
    assert_eq!(a.signal.audio_hash, b.signal.audio_hash);
    // Measurements survive a JSON round trip, and re-assessing them yields
    // the identical assessment.
    let json = a.signal.measurements.to_json().to_string_compact();
    let parsed = Measurements::from_json(&sonora_json::parse(&json).unwrap()).unwrap();
    assert_eq!(parsed.to_json().to_string_compact(), json);
    let again = assess(&a.facts, &parsed, &ScoringConfig::default());
    assert_eq!(again.to_json().to_string_compact(), a.assessment.to_json().to_string_compact());
}

#[test]
fn identical_audio_in_different_containers_shares_audio_hash() {
    let x = quantize16(&track(20.0));
    let a = native(&write("same16.wav", 16, &x));
    let b = native(&write("same24.wav", 24, &x));
    assert_eq!(a.signal.audio_hash, b.signal.audio_hash);
    assert_ne!(a.file_hash, b.file_hash);
    let fp = sonora_engine::analysis::fingerprint::compare_at(&a.signal.fingerprint, &b.signal.fingerprint, 0).unwrap();
    assert_eq!(fp.ber, 0.0);
}

// ------------------------------------------------------ FFmpeg cross-checks

fn ebur128_reference(ffmpeg: &Path, file: &Path) -> (f64, f64, f64) {
    let out = Command::new(ffmpeg)
        .args(["-hide_banner", "-nostats", "-i"])
        .arg(file)
        .args(["-af", "ebur128=peak=true", "-f", "null", "-"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stderr);
    let summary = text.rsplit("Summary:").next().unwrap_or("");
    let grab = |key: &str| -> f64 {
        summary
            .lines()
            .find(|l| l.trim_start().starts_with(key))
            .and_then(|l| l.split(':').nth(1))
            .and_then(|v| v.split_whitespace().next())
            .and_then(|v| v.parse().ok())
            .unwrap_or(f64::NAN)
    };
    (grab("I:"), grab("LRA:"), grab("Peak:"))
}

#[test]
fn loudness_and_true_peak_match_ffmpeg_reference_meter() {
    let Some(ffm) = ffmpeg() else { return };
    let x = track(45.0);
    for (name, data) in [("ref_clean.wav", quantize16(&x)), ("ref_hot.wav", quantize16(&x.iter().map(|v| (v * 1.6).clamp(-1.0, 1.0)).collect::<Vec<_>>()))] {
        let p = write(name, 16, &data);
        let a = native(&p);
        let m = &a.signal.measurements;
        let (i, lra, tp) = ebur128_reference(&ffm, &p);
        let my_i = m.integrated_lufs.unwrap();
        assert!((my_i - i).abs() <= 0.1, "{name}: integrated {my_i} vs ffmpeg {i}");
        assert!((m.lra.unwrap() - lra).abs() <= 0.5, "{name}: LRA {} vs ffmpeg {lra}", m.lra.unwrap());
        assert!((m.true_peak_dbtp - tp).abs() <= 0.3, "{name}: true peak {} vs ffmpeg {tp}", m.true_peak_dbtp);
    }
}

#[test]
fn lossless_float_through_ffmpeg_is_checked_for_clipping() {
    let Some(ffm) = ffmpeg() else { return };
    // Float samples clipped at full scale, stored as float WavPack (decoded by FFmpeg).
    let g = 10f32.powf(5.0 / 20.0);
    let clipped: Vec<f32> = track(30.0).iter().map(|v| (v * g).clamp(-1.0, 1.0)).collect();
    let wav = dir().join("float_clipped.wav");
    write_wav(&wav, FS, 2, 32, true, &clipped).unwrap();
    let wv = dir().join("float_clipped.wv");
    ff(&ffm, &["-i", &wav.to_string_lossy(), "-c:a", "wavpack", &wv.to_string_lossy()]);
    let a = run(&wv, ToolPaths::discover(&[]));
    assert_eq!(a.facts.lossless, Some(true));
    assert!(a.facts.float_samples, "float WavPack is float: {:?}", a.facts.codec_name);
    let c = a.signal.measurements.clipping.as_ref().expect("clipping is measured for lossless float");
    assert!(c.events > 100, "events {}", c.events);
    assert!(find(&a, "clipping").is_some(), "{:?}", ids(&a));
}

#[test]
fn real_lossy_encodes_and_transcodes() {
    let Some(ffm) = ffmpeg() else { return };
    let d = dir();
    let src = write("lossy_src.wav", 16, &quantize16(&track(45.0)));
    let s = src.to_string_lossy().to_string();
    let p = |n: &str| d.join(n).to_string_lossy().to_string();
    ff(&ffm, &["-i", &s, "-c:a", "libmp3lame", "-b:a", "128k", &p("enc128.mp3")]);
    ff(&ffm, &["-i", &s, "-c:a", "libmp3lame", "-b:a", "320k", &p("enc320.mp3")]);
    ff(&ffm, &["-i", &p("enc128.mp3"), "-c:a", "pcm_s16le", &p("transcode128.wav")]);
    ff(&ffm, &["-i", &p("transcode128.wav"), "-c:a", "libmp3lame", "-b:a", "320k", &p("fake320.mp3")]);
    let tools = ToolPaths::discover(&[]);

    let lo = run(&d.join("enc128.mp3"), tools.clone());
    let hi = run(&d.join("enc320.mp3"), tools.clone());
    assert_eq!(lo.facts.codec_id, "mp3");
    assert!(find(&lo, "lossy_codec").unwrap().severity >= Severity::High);
    assert!(find(&lo, "spectral_holes").is_some(), "{:?}", ids(&lo));
    assert!(hi.assessment.scores.overall >= 88.0, "{:?}", hi.assessment.scores);
    assert!(lo.assessment.scores.overall < hi.assessment.scores.overall - 15.0);

    let tc = native(&d.join("transcode128.wav"));
    let f = find(&tc, "lossy_source_suspected").expect("transcode detected");
    assert_eq!(f.evidence, Evidence::Strong);
    // A transcode is never better than its lossy source.
    assert!(tc.assessment.scores.overall <= lo.assessment.scores.overall + 5.0);

    let fake = run(&d.join("fake320.mp3"), tools);
    let f = find(&fake, "bandwidth_below_bitrate").expect("fake 320 detected");
    assert_eq!(f.evidence, Evidence::Strong);
    assert!(fake.assessment.scores.overall < hi.assessment.scores.overall - 15.0);
    assert!(fake.assessment.flags.contains(&"source_concern".to_string()));
}

/// Stored facts and measurements re-score to exactly the same assessment,
/// so a future scoring model can update a library without decoding audio.
#[test]
fn stored_analysis_rescores_identically() {
    use sonora_engine::meta::FileFacts;
    let mut x = track(20.0);
    for v in x.iter_mut() {
        *v = (*v * 1.9).clamp(-1.0, 1.0);
    }
    let a = native(&write("rescore_clipped.wav", 16, &quantize16(&x)));
    let facts_json = sonora_json::parse(&a.facts.to_json().to_string_compact()).unwrap();
    let meas_json = sonora_json::parse(&a.signal.measurements.to_json().to_string_compact()).unwrap();
    let facts = FileFacts::from_json(&facts_json).expect("facts parse");
    let m = Measurements::from_json(&meas_json).expect("measurements parse");
    let again = assess(&facts, &m, &ScoringConfig::default());
    assert_eq!(again.scores.overall, a.assessment.scores.overall);
    assert_eq!(again.scores.dj, a.assessment.scores.dj);
    assert_eq!(again.scores.listening, a.assessment.scores.listening);
    assert_eq!(again.flags, a.assessment.flags);
    assert_eq!(again.verdict, a.assessment.verdict);
    assert_eq!(again.findings.len(), a.assessment.findings.len());
    assert!(again.flags.contains(&"clipping".to_string()), "{:?}", again.flags);
}
