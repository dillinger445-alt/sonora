//! Tempo and key detection on rendered audio, through the full analysis
//! pipeline (WAV written to disk, decoded natively, every meter in one pass).

use sonora_engine::analysis::key::{camelot_distance, format_key, KeyMeter, Notation};
use sonora_engine::analysis::Control;
use sonora_engine::decode::wav::write_wav;
use sonora_engine::decode::ToolPaths;
use sonora_engine::synth::{quantize16, render, white_noise, SynthOptions};
use sonora_engine::{analyze_file, AnalyzeOptions, FileAnalysis};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

fn dir() -> PathBuf {
    let d = std::env::temp_dir().join(format!("sonora-dj-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn analyze(name: &str, samples: &[f32], fs: u32) -> FileAnalysis {
    let p = dir().join(name);
    write_wav(&p, fs, 2, 16, false, &quantize16(samples)).unwrap();
    let cancel = AtomicBool::new(false);
    let mut progress = |_f: f64| {};
    let mut ctl = Control { cancel: &cancel, progress: &mut progress, max_duration_secs: 3600.0 };
    analyze_file(&p, &AnalyzeOptions { tools: ToolPaths::none(), ..Default::default() }, &mut ctl).unwrap()
}

fn synth(seconds: f64, bpm: f64, transpose: f64, major: bool, variant: u32) -> Vec<f32> {
    render(&SynthOptions { seconds, bpm, transpose, major, variant, ..Default::default() })
}

#[test]
fn tempo_of_rendered_tracks() {
    for (bpm, variant) in [(124.0, 0), (128.0, 1), (90.0, 2), (174.0, 0), (140.0, 1), (136.5, 2)] {
        let a = analyze(&format!("tempo_{bpm}.wav"), &synth(50.0, bpm, 0.0, false, variant), 44_100);
        let t = a.signal.measurements.tempo.clone().unwrap_or_else(|| panic!("{bpm}: no tempo"));
        assert!((t.bpm - bpm).abs() < 0.05, "{bpm} BPM read as {}", t.bpm);
        assert!(t.confidence > 0.6, "{bpm}: confidence {}", t.confidence);
        assert!(!t.variable, "{bpm}: {t:?}");
        assert!(t.candidates.iter().any(|c| (c.0 - bpm / 2.0).abs() < 0.05 || (c.0 - bpm * 2.0).abs() < 0.05), "{t:?}");
    }
}

#[test]
fn tempo_at_other_sample_rates() {
    for fs in [48_000, 96_000] {
        let s = render(&SynthOptions { seconds: 40.0, sample_rate: fs, bpm: 126.0, ..Default::default() });
        let t = analyze(&format!("tempo_fs{fs}.wav"), &s, fs).signal.measurements.tempo.unwrap();
        assert!((t.bpm - 126.0).abs() < 0.05, "{fs} Hz: {}", t.bpm);
    }
}

/// Key of interleaved stereo samples at 44.1 kHz, from the key meter alone.
fn key_of(samples: &[f32]) -> Option<u8> {
    let mut m = KeyMeter::new(44_100, 2);
    for c in samples.chunks(16_384 * 2) {
        m.process(c);
    }
    m.finish().map(|k| k.key)
}

/// A cadence (i–iv–V–i or I–IV–V–I, one chord per bar) on a harmonic-rich
/// pad, a root bass, a kick on every beat and off-beat hats.
fn cadence(tonic: u8, minor: bool, bpm: f64, seconds: f64) -> Vec<f32> {
    let fs = 44_100.0;
    let n = (seconds * fs) as usize;
    let chords: [(i32, bool); 4] = if minor { [(0, true), (5, true), (7, false), (0, true)] } else { [(0, false), (5, false), (7, false), (0, false)] };
    let (bar, beat) = (240.0 / bpm, 60.0 / bpm);
    let noise = white_noise(tonic as u64 + 1, n, 1.0);
    let mut out = Vec::with_capacity(n * 2);
    let mut lp = 0.0f64;
    let tau = 2.0 * std::f64::consts::PI;
    for (i, &w) in noise.iter().enumerate() {
        let t = i as f64 / fs;
        let (root, min) = chords[((t / bar) as usize) % 4];
        let mut pad = 0.0;
        for st in [root, root + if min { 3 } else { 4 }, root + 7] {
            let midi = 60 + (tonic as i32 + st).rem_euclid(12);
            let f = 440.0 * 2f64.powf((midi as f64 - 69.0) / 12.0);
            pad += (1..=5).map(|h| (tau * f * h as f64 * t).sin() / h as f64).sum::<f64>();
        }
        let bass_midi = 36 + (tonic as i32 + root).rem_euclid(12);
        let bass = (tau * 440.0 * 2f64.powf((bass_midi as f64 - 69.0) / 12.0) * t).sin() * 0.4;
        let tb = t % beat;
        let kick = (tau * (50.0 * tb + 30.0 * (1.0 - (-tb / 0.03).exp()))).sin() * (-tb / 0.2).exp() * 0.5;
        let x = w as f64 * (-((t + beat / 2.0) % beat) / 0.02).exp() * 0.1;
        let hat = x - lp;
        lp += 0.5 * (x - lp);
        let v = ((pad * 0.06 + bass + kick + hat) * 0.5) as f32;
        out.push(v);
        out.push(v);
    }
    out
}

#[test]
fn key_of_cadences_in_all_24_keys() {
    let mut misses = Vec::new();
    for tonic in 0..12u8 {
        for minor in [false, true] {
            let expect = if minor { 12 + tonic } else { tonic };
            let got = key_of(&cadence(tonic, minor, 120.0 + tonic as f64, 30.0));
            if got != Some(expect) {
                misses.push(format!("{} read as {:?}", format_key(expect, Notation::Musical, true), got.map(|k| format_key(k, Notation::Musical, true))));
            }
        }
    }
    assert!(misses.len() <= 2, "{misses:?}");
}

#[test]
fn key_of_rendered_tracks() {
    // A minor (Am7 pad) and C major (Cmaj7 pad) moved round the wheel, with
    // three bass lines each; some lean on the dominant or the relative key.
    let (mut exact, mut compatible, mut total) = (0, 0, 0);
    let mut misses = Vec::new();
    for (i, tr) in [0.0, 3.0, -2.0, 5.0, -4.0, 1.0, 6.0, -5.0].iter().enumerate() {
        for major in [false, true] {
            for variant in 0..3 {
                let s = synth(30.0, 120.0 + i as f64, *tr, major, variant);
                let expect = if major { (0.0 + tr).rem_euclid(12.0) as u8 } else { 12 + (9.0 + tr).rem_euclid(12.0) as u8 };
                let got = key_of(&s);
                total += 1;
                if got == Some(expect) {
                    exact += 1;
                } else {
                    misses.push(format!("{} (variant {variant}) read as {:?}", format_key(expect, Notation::Musical, true), got.map(|k| format_key(k, Notation::Musical, true))));
                }
                if got.is_some_and(|g| camelot_distance(g, expect) <= 1) {
                    compatible += 1;
                }
            }
        }
    }
    assert!(exact * 100 >= total * 80, "{exact}/{total} exact: {misses:?}");
    assert!(compatible * 100 >= total * 90, "{compatible}/{total} compatible: {misses:?}");
}

#[test]
fn key_through_the_whole_pipeline() {
    for (tr, major, expect) in [(0.0, false, "Am"), (3.0, true, "Eb")] {
        let a = analyze(&format!("key_{expect}.wav"), &synth(40.0, 124.0, tr, major, if major { 1 } else { 0 }), 44_100);
        let k = a.signal.measurements.key.clone().unwrap_or_else(|| panic!("{expect}: no key"));
        assert_eq!(format_key(k.key, Notation::Musical, true), expect, "{k:?}");
        assert!(k.tuning_cents.abs() < 10.0, "{k:?}");
        assert!(!k.changes, "{k:?}");
        assert!(k.confidence > 0.2, "{k:?}");
    }
}

#[test]
fn tuning_off_concert_pitch_is_measured_and_tolerated() {
    // A = 432 Hz: 31.8 cents flat.
    let shift = 12.0 * (432.0f64 / 440.0).log2();
    let a = analyze("a432.wav", &synth(40.0, 122.0, shift, false, 0), 44_100);
    let k = a.signal.measurements.key.unwrap();
    assert_eq!(format_key(k.key, Notation::Musical, true), "Am");
    assert!((k.tuning_cents + 31.8).abs() < 6.0, "{k:?}");
}

#[test]
fn noise_has_neither_tempo_nor_key() {
    let n = white_noise(7, 44_100 * 2 * 30, 0.3);
    let m = analyze("noise.wav", &n, 44_100).signal.measurements;
    assert!(m.tempo.is_none(), "{:?}", m.tempo);
    assert!(m.key.is_none(), "{:?}", m.key);
}

#[test]
fn sustained_chord_has_a_key_but_no_tempo() {
    let fs = 44_100.0;
    let n = (30.0 * fs) as usize;
    let mut s = Vec::with_capacity(n * 2);
    // D minor triad with a D bass, slowly swelling.
    let notes = [146.83, 174.61, 220.0, 293.66, 73.42];
    for i in 0..n {
        let t = i as f64 / fs;
        let env = 0.6 + 0.4 * (2.0 * std::f64::consts::PI * 0.05 * t).sin();
        let v: f64 = notes.iter().map(|f| (2.0 * std::f64::consts::PI * f * t).sin() + 0.3 * (4.0 * std::f64::consts::PI * f * t).sin()).sum::<f64>() * 0.08 * env;
        s.push(v as f32);
        s.push(v as f32);
    }
    let m = analyze("chord.wav", &s, 44_100).signal.measurements;
    assert!(m.tempo.is_none(), "{:?}", m.tempo);
    let k = m.key.unwrap();
    assert_eq!(format_key(k.key, Notation::Musical, true), "Dm", "{k:?}");
}

#[test]
fn a_tempo_change_is_flagged() {
    // First half at 122 BPM, second half at 132 BPM.
    let a = synth(120.0, 122.0, 0.0, false, 0);
    let b = synth(120.0, 132.0, 0.0, false, 0);
    let mut s = a[..a.len() / 2].to_vec();
    s.extend_from_slice(&b[b.len() / 2..]);
    let t = analyze("tempo_change.wav", &s, 44_100).signal.measurements.tempo.unwrap();
    assert!(t.variable, "{t:?}");
    assert!(t.bpm > 120.0 && t.bpm < 134.0, "{t:?}");
}
