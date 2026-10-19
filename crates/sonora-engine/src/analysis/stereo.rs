//! Stereo image and mono-compatibility measurements (first two channels).
//!
//! Club systems commonly sum the low end to mono, so out-of-phase bass is a
//! real playback risk; this module measures full-band and low-band
//! (< 120 Hz) correlation and the level change on a mono fold-down.

use crate::dsp::biquad::{BiquadCoef, BiquadState};

const LOW_BAND_HZ: f64 = 120.0;
/// Low-band analysis block length in seconds.
const BLOCK_SECS: f64 = 0.2;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct StereoResult {
    /// Pearson correlation of L and R over the whole file (-1 … 1).
    pub correlation: f64,
    /// Correlation of the low band (< 120 Hz).
    pub low_correlation: Option<f64>,
    /// Level change of a mono fold-down ((L+R)/2) relative to the average
    /// channel level, in dB (0 dB for identical channels, ≈ -3 dB for
    /// uncorrelated, strongly negative for out-of-phase content).
    pub mono_loss_db: f64,
    pub low_mono_loss_db: Option<f64>,
    /// Fraction of bass-carrying blocks whose low-band correlation is negative.
    pub low_negative_fraction: f64,
    /// Side energy relative to mid energy (dB); -inf for identical channels.
    pub side_to_mid_db: f64,
    /// The two channels are sample-identical.
    pub identical: bool,
    /// The channels are identical except for polarity (R = -L).
    pub inverted: bool,
    pub max_abs_difference: f32,
}

pub struct StereoMeter {
    channels: usize,
    lp: BiquadCoef,
    st: [BiquadState; 2],
    sum_ll: f64,
    sum_rr: f64,
    sum_lr: f64,
    low_ll: f64,
    low_rr: f64,
    low_lr: f64,
    max_diff: f32,
    max_sum: f32,
    block_len: usize,
    block_pos: usize,
    blk: [f64; 3],
    bass_blocks: u64,
    negative_blocks: u64,
}

impl StereoMeter {
    pub fn new(sample_rate: u32, channels: usize) -> Self {
        let fs = sample_rate as f64;
        StereoMeter {
            channels,
            lp: BiquadCoef::lowpass(fs, LOW_BAND_HZ, std::f64::consts::FRAC_1_SQRT_2),
            st: [BiquadState::default(), BiquadState::default()],
            sum_ll: 0.0,
            sum_rr: 0.0,
            sum_lr: 0.0,
            low_ll: 0.0,
            low_rr: 0.0,
            low_lr: 0.0,
            max_diff: 0.0,
            max_sum: 0.0,
            block_len: ((fs * BLOCK_SECS) as usize).max(1),
            block_pos: 0,
            blk: [0.0; 3],
            bass_blocks: 0,
            negative_blocks: 0,
        }
    }

    pub fn active(&self) -> bool {
        self.channels >= 2
    }

    pub fn process(&mut self, interleaved: &[f32]) {
        if self.channels < 2 {
            return;
        }
        let ch = self.channels;
        // A block carries bass when its low-band RMS exceeds -50 dBFS.
        let bass_threshold = 1e-5;
        for frame in interleaved.chunks_exact(ch) {
            let (l, r) = (frame[0], frame[1]);
            let (ld, rd) = (l as f64, r as f64);
            self.sum_ll += ld * ld;
            self.sum_rr += rd * rd;
            self.sum_lr += ld * rd;
            self.max_diff = self.max_diff.max((l - r).abs());
            self.max_sum = self.max_sum.max((l + r).abs());
            let fl = self.st[0].process(&self.lp, ld);
            let fr = self.st[1].process(&self.lp, rd);
            self.low_ll += fl * fl;
            self.low_rr += fr * fr;
            self.low_lr += fl * fr;
            self.blk[0] += fl * fl;
            self.blk[1] += fr * fr;
            self.blk[2] += fl * fr;
            self.block_pos += 1;
            if self.block_pos == self.block_len {
                let n = self.block_len as f64;
                let (bl, br, blr) = (self.blk[0] / n, self.blk[1] / n, self.blk[2] / n);
                if (bl + br) * 0.5 > bass_threshold {
                    self.bass_blocks += 1;
                    let denom = (bl * br).sqrt();
                    if denom > 0.0 && blr / denom < 0.0 {
                        self.negative_blocks += 1;
                    }
                }
                self.blk = [0.0; 3];
                self.block_pos = 0;
            }
        }
    }

    pub fn finish(&self) -> Option<StereoResult> {
        if self.channels < 2 {
            return None;
        }
        let corr = |ll: f64, rr: f64, lr: f64| -> Option<f64> {
            let d = (ll * rr).sqrt();
            if d > 1e-20 {
                Some((lr / d).clamp(-1.0, 1.0))
            } else {
                None
            }
        };
        let mono_loss = |ll: f64, rr: f64, lr: f64| -> Option<f64> {
            let avg = (ll + rr) / 2.0;
            if avg <= 1e-20 {
                return None;
            }
            let mono = (ll + rr + 2.0 * lr) / 4.0;
            Some(10.0 * (mono.max(1e-30) / avg).log10())
        };
        let mid = (self.sum_ll + self.sum_rr + 2.0 * self.sum_lr) / 4.0;
        let side = (self.sum_ll + self.sum_rr - 2.0 * self.sum_lr) / 4.0;
        let low_energy = (self.low_ll + self.low_rr) / 2.0;
        let total_energy = (self.sum_ll + self.sum_rr) / 2.0;
        // Low band is only meaningful when there is real bass content.
        let has_bass = total_energy > 0.0 && low_energy > total_energy * 1e-3;
        Some(StereoResult {
            correlation: corr(self.sum_ll, self.sum_rr, self.sum_lr).unwrap_or(1.0),
            low_correlation: if has_bass { corr(self.low_ll, self.low_rr, self.low_lr) } else { None },
            mono_loss_db: mono_loss(self.sum_ll, self.sum_rr, self.sum_lr).unwrap_or(0.0),
            low_mono_loss_db: if has_bass { mono_loss(self.low_ll, self.low_rr, self.low_lr) } else { None },
            low_negative_fraction: if self.bass_blocks > 0 { self.negative_blocks as f64 / self.bass_blocks as f64 } else { 0.0 },
            side_to_mid_db: if mid > 0.0 { 10.0 * (side.max(1e-30) / mid).log10() } else { 0.0 },
            identical: self.max_diff == 0.0 && total_energy > 0.0,
            inverted: self.max_sum == 0.0 && total_energy > 0.0,
            max_abs_difference: self.max_diff,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    fn run(f: impl Fn(f64) -> (f32, f32)) -> StereoResult {
        let fs = 44_100;
        let mut x = Vec::new();
        for i in 0..fs * 2 {
            let (l, r) = f(i as f64 / fs as f64);
            x.push(l);
            x.push(r);
        }
        let mut m = StereoMeter::new(fs as u32, 2);
        m.process(&x);
        m.finish().unwrap()
    }

    #[test]
    fn identical_channels() {
        let r = run(|t| {
            let v = (0.5 * (2.0 * PI * 60.0 * t).sin()) as f32;
            (v, v)
        });
        assert!(r.identical);
        assert!((r.correlation - 1.0).abs() < 1e-9);
        assert!(r.mono_loss_db.abs() < 1e-6);
    }

    #[test]
    fn inverted_bass_is_flagged() {
        let r = run(|t| {
            let v = (0.5 * (2.0 * PI * 50.0 * t).sin()) as f32;
            (v, -v)
        });
        assert!(r.inverted);
        assert!(r.correlation < -0.99);
        assert!(r.low_correlation.unwrap() < -0.99);
        assert!(r.low_negative_fraction > 0.95);
        assert!(r.low_mono_loss_db.unwrap() < -40.0);
    }

    #[test]
    fn wide_highs_with_mono_bass() {
        // Mono 50 Hz bass + decorrelated high-frequency content.
        let r = run(|t| {
            let bass = 0.4 * (2.0 * PI * 50.0 * t).sin();
            let hl = 0.2 * (2.0 * PI * 5000.0 * t).sin();
            let hr = 0.2 * (2.0 * PI * 5000.0 * t + PI / 2.0).sin();
            ((bass + hl) as f32, (bass + hr) as f32)
        });
        assert!(!r.identical);
        assert!(r.low_correlation.unwrap() > 0.95);
        assert!(r.low_negative_fraction < 0.05);
        assert!(r.correlation > 0.5);
    }
}
