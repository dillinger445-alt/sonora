//! Deterministic "electronic track" synthesiser used by the test suite, the
//! CLI fixture generator and the development demo library. The goal is realistic *technical* properties — full-bandwidth
//! hi-hats and cymbals, sub-bass, stereo pads, an arrangement with a
//! breakdown — not musical quality. Everything derives from a seed, so
//! fixtures are bit-for-bit reproducible.

use crate::dsp::biquad::{BiquadCoef, BiquadState};
use std::f64::consts::PI;

#[derive(Clone, Debug)]
pub struct SynthOptions {
    pub seconds: f64,
    pub sample_rate: u32,
    pub seed: u64,
    pub bpm: f64,
    /// Overall output peak after mastering (dBFS).
    pub peak_dbfs: f64,
    /// Pattern variation (changes bass line and hat pattern).
    pub variant: u32,
    /// Semitones added to every pitched part (bass and pads).
    pub transpose: f64,
    /// Major-key harmony (a major-seventh pad over I–V–vi–IV roots, C major
    /// before transposing) instead of the default A minor.
    pub major: bool,
}

impl Default for SynthOptions {
    fn default() -> Self {
        SynthOptions { seconds: 45.0, sample_rate: 44_100, seed: 1, bpm: 124.0, peak_dbfs: -1.0, variant: 0, transpose: 0.0, major: false }
    }
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 11) as f64 / (1u64 << 53) as f64) * 2.0 - 1.0
    }
}

/// Two cascaded biquads (4th order).
struct Filter4 {
    c: BiquadCoef,
    s: [BiquadState; 2],
}

impl Filter4 {
    fn new(c: BiquadCoef) -> Self {
        Filter4 { c, s: Default::default() }
    }
    fn run(&mut self, x: f64) -> f64 {
        let y = self.s[0].process(&self.c, x);
        self.s[1].process(&self.c, y)
    }
}

fn bandpass(fs: f64, f0: f64, q: f64) -> BiquadCoef {
    let w = 2.0 * PI * f0 / fs;
    let (sw, cw) = w.sin_cos();
    let alpha = sw / (2.0 * q);
    let a0 = 1.0 + alpha;
    BiquadCoef { b0: alpha / a0, b1: 0.0, b2: -alpha / a0, a1: -2.0 * cw / a0, a2: (1.0 - alpha) / a0 }
}

fn saw(phase: f64) -> f64 {
    2.0 * (phase - phase.floor()) - 1.0
}

/// Renders interleaved stereo `f32` samples.
pub fn render(o: &SynthOptions) -> Vec<f32> {
    let fs = o.sample_rate as f64;
    let n = (o.seconds * fs) as usize;
    let beat = 60.0 / o.bpm;
    let sixteenth = beat / 4.0;
    let mut rng = Rng(o.seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let q = std::f64::consts::FRAC_1_SQRT_2;

    let mut hat_l = Filter4::new(BiquadCoef::highpass(fs, 7000.0, q));
    let mut hat_r = Filter4::new(BiquadCoef::highpass(fs, 7000.0, q));
    let mut ohat = Filter4::new(BiquadCoef::highpass(fs, 5500.0, q));
    let mut clap_l = Filter4::new(bandpass(fs, 1300.0, 1.2));
    let mut clap_r = Filter4::new(bandpass(fs, 1450.0, 1.2));
    let mut ride = Filter4::new(BiquadCoef::highpass(fs, 4500.0, q));
    let mut air = Filter4::new(BiquadCoef::highpass(fs, 9000.0, q));
    let mut bass_lp = Filter4::new(BiquadCoef::lowpass(fs, 260.0, 0.9));
    let mut click = Filter4::new(BiquadCoef::highpass(fs, 2500.0, q));
    let mut pad_l = [BiquadState::default(), BiquadState::default()];
    let mut pad_r = [BiquadState::default(), BiquadState::default()];

    // A minor (Am7 pad) by default; C major (Cmaj7 pad) when asked.
    let bass_lines: [[f64; 4]; 3] = if o.major {
        [[65.41, 49.0, 55.0, 43.65], [65.41, 43.65, 49.0, 65.41], [65.41, 55.0, 43.65, 49.0]]
    } else {
        [[55.0, 55.0, 65.41, 49.0], [41.2, 55.0, 49.0, 61.74], [55.0, 73.42, 65.41, 55.0]]
    };
    let chord = if o.major { [261.63, 329.63, 392.0, 493.88] } else { [220.0, 261.63, 329.63, 392.0] };
    // Exactly 1.0 without transposition, so default renders are unchanged.
    let ratio = 2f64.powf(o.transpose / 12.0);
    let bass_notes = bass_lines[(o.variant % 3) as usize].map(|f| f * ratio);
    let chord = chord.map(|f| f * ratio);
    let ride_freqs = [2520.0, 3215.0, 4710.0, 5940.0, 7130.0, 8390.0];

    let mut kick_phase = 0.0;
    let mut bass_phase = 0.0;
    let mut pad_phases = [[0.0f64; 3]; 8];
    let mut ride_phases = [0.0f64; 6];
    let mut out = Vec::with_capacity(n * 2);
    let mut raw: Vec<(f64, f64)> = Vec::with_capacity(n);

    for i in 0..n {
        let t = i as f64 / fs;
        let pos = t / o.seconds;
        // Arrangement.
        let intro = pos < 0.15;
        let breakdown = (0.55..0.70).contains(&pos);
        let drums = !breakdown;
        let full = !intro && !breakdown;

        let tb = t % beat;
        let ts = t % sixteenth;
        let step = ((t / sixteenth) as u64) % 16;
        let bar = (t / (beat * 4.0)) as u64;

        let mut l = 0.0;
        let mut r = 0.0;

        // Kick: pitch-swept sine with a short click.
        if drums {
            let f = 45.0 + 110.0 * (-tb / 0.03).exp();
            kick_phase += f / fs;
            let k = (2.0 * PI * kick_phase).sin() * (-tb / 0.28).exp() * 0.95;
            let c = click.run(if tb < 0.002 { rng.next() } else { 0.0 }) * 0.25;
            l += k + c;
            r += k + c;
        }

        // Bass on off-beat sixteenths.
        if full || breakdown && pos > 0.65 {
            let note = bass_notes[((t / beat) as usize) % 4];
            bass_phase += note / fs;
            let gate = matches!(step % 4, 2 | 3) as u8 as f64;
            let env = gate * (-(ts) / 0.12).exp();
            let b = bass_lp.run(saw(bass_phase) * env) * 0.55;
            l += b;
            r += b;
        }

        // Closed hats: off-beat eighths, quieter sixteenths (variant-dependent).
        if drums {
            let accent = if step % 4 == 2 {
                1.0
            } else if o.variant % 2 == 1 && step % 2 == 1 {
                0.35
            } else {
                0.0
            };
            let env = accent * (-ts / 0.035).exp();
            let pan = if step % 8 < 4 { 0.85 } else { 1.0 };
            l += hat_l.run(rng.next()) * env * 0.35 * pan;
            r += hat_r.run(rng.next()) * env * 0.35 * (1.85 - pan);
            // Open hat on the last off-beat of each bar.
            if step == 14 && !intro {
                let oh = ohat.run(rng.next()) * (-ts / 0.22).exp() * 0.25;
                l += oh * 0.9;
                r += oh;
            }
        }

        // Clap on 2 and 4.
        if full {
            let beat_in_bar = ((t / beat) as u64) % 4;
            if beat_in_bar == 1 || beat_in_bar == 3 {
                let bursts = (tb < 0.008) || (tb > 0.011 && tb < 0.019) || (tb > 0.022 && tb < 0.03);
                let env = if bursts { 1.0 } else { (-(tb - 0.03).max(0.0) / 0.12).exp() * 0.6 };
                l += clap_l.run(rng.next()) * env * 0.5;
                r += clap_r.run(rng.next()) * env * 0.5;
            }
        }

        // Metallic ride on quarter notes in every other bar (and through the breakdown).
        if (bar % 2 == 1 && !intro) || breakdown {
            let mut m = 0.0;
            for (p, f) in ride_phases.iter_mut().zip(ride_freqs.iter()) {
                *p += f / fs;
                m += if (*p).fract() < 0.5 { 1.0 } else { -1.0 };
            }
            let v = ride.run(m / 6.0 + 0.3 * rng.next()) * (-tb / 0.5).exp() * 0.16;
            l += v * 1.1;
            r += v * 0.9;
        } else {
            // Keep the filter state moving.
            ride.run(0.0);
        }

        // Pads: detuned saws, slowly moving low-pass, wide stereo.
        if !intro {
            let cutoff = 900.0 + 2600.0 * (0.5 + 0.5 * (2.0 * PI * 0.05 * t).sin());
            let lp = BiquadCoef::lowpass(fs, cutoff, 0.8);
            let mut pl = 0.0;
            let mut pr = 0.0;
            for (ni, &f) in chord.iter().enumerate() {
                // Three detuned voices per note, indexed in two phase banks.
                #[allow(clippy::needless_range_loop)]
                for d in 0..3 {
                    let det = 1.0 + (d as f64 - 1.0) * 0.004;
                    pad_phases[ni][d] += f * det / fs;
                    pad_phases[ni + 4][d] += f * (2.0 - det) / fs;
                    pl += saw(pad_phases[ni][d]);
                    pr += saw(pad_phases[ni + 4][d]);
                }
            }
            let level = if breakdown { 0.07 } else { 0.035 };
            let y1 = pad_l[0].process(&lp, pl * level);
            l += pad_l[1].process(&lp, y1);
            let y2 = pad_r[0].process(&lp, pr * level);
            r += pad_r[1].process(&lp, y2);
        }

        // A little constant "air" so the very top octave is never empty.
        let a = air.run(rng.next()) * 0.0012;
        l += a;
        r += a * 0.8 + rng.next() * 0.0002;
        raw.push((l, r));
    }

    // Master: gentle saturation, then normalise to the requested peak.
    let drive = 1.4;
    let sat = |x: f64| (x * drive).tanh() / drive.tanh();
    let mut peak = 0f64;
    for (l, r) in raw.iter_mut() {
        *l = sat(*l * 0.8);
        *r = sat(*r * 0.8);
        peak = peak.max(l.abs()).max(r.abs());
    }
    let gain = 10f64.powf(o.peak_dbfs / 20.0) / peak.max(1e-9);
    for (l, r) in raw {
        out.push((l * gain) as f32);
        out.push((r * gain) as f32);
    }
    out
}

/// Deterministic white noise in [-amp, amp).
pub fn white_noise(seed: u64, n: usize, amp: f64) -> Vec<f32> {
    let mut r = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    (0..n).map(|_| (r.next() * amp) as f32).collect()
}

/// Rounds samples to 16-bit resolution (as a 16-bit file would store them).
pub fn quantize16(x: &[f32]) -> Vec<f32> {
    x.iter().map(|&v| ((v as f64 * 32768.0).round().clamp(-32768.0, 32767.0) / 32768.0) as f32).collect()
}
