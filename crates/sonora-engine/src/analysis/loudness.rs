//! Loudness per ITU-R BS.1770-4 / EBU R128 (Tech 3341, Tech 3342).
//!
//! K-weighted mean squares are accumulated in 100 ms blocks; momentary
//! (400 ms) and short-term (3 s) loudness are sliding sums of those blocks.
//! Integrated loudness uses the two-stage gate (-70 LUFS absolute, -10 LU
//! relative) over 400 ms blocks with 75 % overlap; loudness range (LRA) is
//! the 10th–95th percentile spread of short-term loudness after a -70 LUFS
//! absolute and -20 LU relative gate.

use crate::dsp::biquad::{BiquadCoef, BiquadState};
use crate::dsp::kweight;

const ABS_GATE_LUFS: f64 = -70.0;

#[inline]
fn power_to_lufs(p: f64) -> f64 {
    if p <= 0.0 {
        f64::NEG_INFINITY
    } else {
        -0.691 + 10.0 * p.log10()
    }
}

#[inline]
fn lufs_to_power(l: f64) -> f64 {
    10f64.powf((l + 0.691) / 10.0)
}

pub struct LoudnessMeter {
    channels: usize,
    sample_rate: u32,
    pre: BiquadCoef,
    rlb: BiquadCoef,
    state: Vec<(BiquadState, BiquadState)>,
    weights: Vec<f64>,
    block_len: usize,
    block_pos: usize,
    block_acc: Vec<f64>,
    /// Channel-weighted mean square of each complete 100 ms block.
    blocks: Vec<f64>,
}

#[derive(Clone, Debug, Default)]
pub struct LoudnessResult {
    /// Integrated loudness (LUFS); `None` if every block is below the absolute gate.
    pub integrated: Option<f64>,
    /// Loudness range (LU).
    pub lra: Option<f64>,
    pub lra_low: Option<f64>,
    pub lra_high: Option<f64>,
    pub max_momentary: Option<f64>,
    pub max_short_term: Option<f64>,
    /// Short-term loudness sampled every `timeline_step` seconds (None = below -70 LUFS).
    /// Value `i` is the loudest window that starts within
    /// `[i * timeline_step, (i + 1) * timeline_step)`.
    pub timeline: Vec<Option<f64>>,
    pub timeline_step: f64,
    /// Length of each window in the timeline (s): 3 for short-term loudness,
    /// 0.4 for the momentary loudness used when the file is shorter than 3 s.
    pub timeline_window: f64,
    /// Fraction of 400 ms blocks that pass the absolute gate.
    pub gated_fraction: f64,
}

impl LoudnessMeter {
    pub fn new(sample_rate: u32, channels: usize) -> Self {
        let fs = sample_rate as f64;
        LoudnessMeter {
            channels,
            sample_rate,
            pre: kweight::pre_filter(fs),
            rlb: kweight::rlb_filter(fs),
            state: vec![(BiquadState::default(), BiquadState::default()); channels],
            weights: (0..channels).map(|i| kweight::channel_weight(i, channels)).collect(),
            block_len: ((sample_rate as f64 * 0.1).round() as usize).max(1),
            block_pos: 0,
            block_acc: vec![0.0; channels],
            blocks: Vec::new(),
        }
    }

    pub fn process(&mut self, interleaved: &[f32]) {
        let ch = self.channels;
        if ch == 0 {
            return;
        }
        let frames = interleaved.len() / ch;
        let mut start = 0;
        while start < frames {
            let take = (self.block_len - self.block_pos).min(frames - start);
            for c in 0..ch {
                if self.weights[c] == 0.0 {
                    continue;
                }
                let (s1, s2) = &mut self.state[c];
                let mut acc = 0f64;
                for f in start..start + take {
                    let x = interleaved[f * ch + c] as f64;
                    let y = s2.process(&self.rlb, s1.process(&self.pre, x));
                    acc += y * y;
                }
                self.block_acc[c] += acc;
            }
            self.block_pos += take;
            start += take;
            if self.block_pos == self.block_len {
                let n = self.block_len as f64;
                let p: f64 = self.block_acc.iter().zip(&self.weights).map(|(a, w)| w * a / n).sum();
                self.blocks.push(p);
                self.block_acc.iter_mut().for_each(|a| *a = 0.0);
                self.block_pos = 0;
            }
        }
    }

    /// Number of complete 100 ms blocks.
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    /// Mean-square power of each 100 ms block (K-weighted, channel-weighted).
    pub fn blocks(&self) -> &[f64] {
        &self.blocks
    }

    fn windowed(&self, len: usize) -> Vec<f64> {
        if self.blocks.len() < len {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(self.blocks.len() - len + 1);
        let mut acc: f64 = self.blocks[..len].iter().sum();
        out.push(acc / len as f64);
        for i in len..self.blocks.len() {
            acc += self.blocks[i] - self.blocks[i - len];
            out.push(acc.max(0.0) / len as f64);
        }
        out
    }

    pub fn finish(&self, max_timeline_points: usize) -> LoudnessResult {
        let mut r = LoudnessResult::default();
        let momentary = self.windowed(4);
        let short = self.windowed(30);

        // Integrated loudness.
        let abs_gate_p = lufs_to_power(ABS_GATE_LUFS);
        let gated: Vec<f64> = momentary.iter().copied().filter(|&p| p > abs_gate_p).collect();
        if !momentary.is_empty() {
            r.gated_fraction = gated.len() as f64 / momentary.len() as f64;
        }
        if !gated.is_empty() {
            let mean = gated.iter().sum::<f64>() / gated.len() as f64;
            let rel_gate = power_to_lufs(mean) - 10.0;
            let rel_p = lufs_to_power(rel_gate);
            let kept: Vec<f64> = gated.iter().copied().filter(|&p| p > rel_p).collect();
            if !kept.is_empty() {
                r.integrated = Some(power_to_lufs(kept.iter().sum::<f64>() / kept.len() as f64));
            }
        }
        r.max_momentary = momentary.iter().copied().fold(None, |m: Option<f64>, p| Some(m.map_or(p, |m| m.max(p)))).map(power_to_lufs).filter(|l| l.is_finite());

        // Loudness range from short-term loudness.
        let st_gated: Vec<f64> = short.iter().copied().filter(|&p| p > abs_gate_p).collect();
        if !st_gated.is_empty() {
            let mean = st_gated.iter().sum::<f64>() / st_gated.len() as f64;
            let rel_gate = power_to_lufs(mean) - 20.0;
            let mut vals: Vec<f64> = st_gated.iter().map(|&p| power_to_lufs(p)).filter(|&l| l > rel_gate).collect();
            vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            if vals.len() >= 2 {
                let lo = crate::util::percentile_sorted(&vals, 10.0).unwrap_or(0.0);
                let hi = crate::util::percentile_sorted(&vals, 95.0).unwrap_or(0.0);
                r.lra = Some((hi - lo).max(0.0));
                r.lra_low = Some(lo);
                r.lra_high = Some(hi);
            }
        }
        r.max_short_term = short.iter().copied().fold(None, |m: Option<f64>, p| Some(m.map_or(p, |m| m.max(p)))).map(power_to_lufs).filter(|l| l.is_finite());

        // Timeline: short-term loudness (or momentary for very short files),
        // reduced to at most `max_timeline_points` points by taking the
        // maximum power in each bucket.
        let series = if short.is_empty() { &momentary } else { &short };
        if !series.is_empty() && max_timeline_points > 0 {
            let per = series.len().div_ceil(max_timeline_points).max(1);
            r.timeline_step = per as f64 * 0.1;
            r.timeline_window = if short.is_empty() { 0.4 } else { 3.0 };
            r.timeline = series
                .chunks(per)
                .map(|c| {
                    let p = c.iter().copied().fold(0.0, f64::max);
                    let l = power_to_lufs(p);
                    if l > ABS_GATE_LUFS {
                        Some(crate::util::round_to(l, 1))
                    } else {
                        None
                    }
                })
                .collect();
        }
        let _ = self.sample_rate;
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    fn stereo_sine(fs: u32, f: f64, dbfs: f64, secs: f64) -> Vec<f32> {
        let a = 10f64.powf(dbfs / 20.0);
        let n = (fs as f64 * secs) as usize;
        let mut v = Vec::with_capacity(n * 2);
        for i in 0..n {
            let s = (a * (2.0 * PI * f * i as f64 / fs as f64).sin()) as f32;
            v.push(s);
            v.push(s);
        }
        v
    }

    #[test]
    fn ebu_tech3341_case1_stereo_1khz_at_minus_23() {
        // EBU Tech 3341 test 1: stereo 1 kHz sine at -23 dBFS → -23.0 LUFS (±0.1).
        for fs in [44_100u32, 48_000, 96_000] {
            let x = stereo_sine(fs, 1000.0, -23.0, 20.0);
            let mut m = LoudnessMeter::new(fs, 2);
            for c in x.chunks(4096 * 2) {
                m.process(c);
            }
            let r = m.finish(100);
            let i = r.integrated.unwrap();
            assert!((i + 23.0).abs() < 0.1, "fs {fs}: {i}");
            // A steady tone has (almost) no loudness range.
            assert!(r.lra.unwrap() < 0.2);
        }
    }

    #[test]
    fn relative_gate_and_lra() {
        // 20 s at -36 dBFS followed by 20 s at -16 dBFS (EBU 3341 case 3 shape):
        // the quiet part sits 20 dB below and is removed by the relative gate.
        let fs = 48_000;
        let mut x = stereo_sine(fs, 1000.0, -36.0, 20.0);
        x.extend(stereo_sine(fs, 1000.0, -16.0, 20.0));
        let mut m = LoudnessMeter::new(fs, 2);
        m.process(&x);
        let r = m.finish(50);
        assert!((r.integrated.unwrap() + 16.0).abs() < 0.15, "{:?}", r.integrated);
        // EBU 3342 case 2 style: -30/-10 alternating → LRA ≈ 20 LU; here the
        // 20 LU step is visible in the short-term distribution.
        let lra = r.lra.unwrap();
        assert!(lra > 15.0 && lra < 21.0, "lra {lra}");
        assert!(r.timeline.len() <= 50);
    }

    #[test]
    fn silence_has_no_integrated_loudness() {
        let x = vec![0f32; 48_000 * 2 * 5];
        let mut m = LoudnessMeter::new(48_000, 2);
        m.process(&x);
        let r = m.finish(10);
        assert!(r.integrated.is_none());
        assert!(r.lra.is_none());
    }
}
