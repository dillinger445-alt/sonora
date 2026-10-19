//! True-peak (inter-sample peak) estimation, following the method of
//! ITU-R BS.1770-4 Annex 2: oversample (4x below 88.2 kHz, 2x below
//! 176.4 kHz, none above) with a polyphase interpolation filter and take
//! the absolute maximum.
//!
//! The interpolator is a Kaiser-windowed sinc with 16 taps per phase (longer
//! than the 12-tap example filter in the recommendation), with every phase
//! normalised to unity DC gain. Accuracy is better than ±0.1 dB for content
//! below 0.4·fs and within the recommendation's tolerance up to 0.45·fs.

use crate::util::{thin_evenly, SpreadTimes};

const TAPS: usize = 16;
const KAISER_BETA: f64 = 7.0;

fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let q = x * x / 4.0;
    for k in 1..64 {
        term *= q / (k as f64 * k as f64);
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}

fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-12 {
        1.0
    } else {
        let px = std::f64::consts::PI * x;
        px.sin() / px
    }
}

/// Oversampling factor used for a given sample rate.
pub fn oversampling_factor(sample_rate: u32) -> usize {
    if sample_rate < 88_200 {
        4
    } else if sample_rate < 176_400 {
        2
    } else {
        1
    }
}

/// Builds the fractional-delay phases (excluding the zero phase, which is
/// the input sample itself). Weights are ordered oldest → newest sample.
fn design_phases(factor: usize) -> Vec<[f32; TAPS]> {
    let half = TAPS as f64 / 2.0;
    let i0b = bessel_i0(KAISER_BETA);
    (1..factor)
        .map(|p| {
            let d = p as f64 / factor as f64;
            let mut w = [0f64; TAPS];
            for (i, wi) in w.iter_mut().enumerate() {
                // Distance (in samples) from the interpolation point to sample i.
                let u = (half - 1.0 - i as f64) + d;
                let r = (u / half).clamp(-1.0, 1.0);
                let kaiser = bessel_i0(KAISER_BETA * (1.0 - r * r).max(0.0).sqrt()) / i0b;
                *wi = sinc(u) * kaiser;
            }
            let sum: f64 = w.iter().sum();
            let mut out = [0f32; TAPS];
            for (o, wi) in out.iter_mut().zip(w.iter()) {
                *o = (wi / sum) as f32;
            }
            out
        })
        .collect()
}

/// Samples per gating block. Must be at least `TAPS`.
const BLOCK: usize = 64;

struct ChannelState {
    buf: [f32; 2 * TAPS],
    pos: usize,
    max: f32,
    /// Largest |x| in the previous block (its tail is still in the window).
    prev_block_max: f32,
    /// Frame index (per channel) of the most recent over, for event grouping.
    last_over: Option<u64>,
    frames: u64,
    over_count: u64,
    over_events: SpreadTimes,
    over_event_total: u64,
}

/// Streaming true-peak meter for interleaved audio.
///
/// Interpolation is skipped for blocks whose samples provably cannot
/// produce a new maximum or an over: an interpolated value never exceeds
/// `gain` (the largest absolute tap sum) times the largest neighbouring
/// sample. This is exact, not an approximation, and makes typical music
/// several times cheaper to measure.
pub struct TruePeakMeter {
    channels: usize,
    phases: Vec<[f32; TAPS]>,
    gain: f32,
    state: Vec<ChannelState>,
    sample_rate: u32,
    max_events: usize,
}

impl TruePeakMeter {
    pub fn new(sample_rate: u32, channels: usize) -> Self {
        let factor = oversampling_factor(sample_rate);
        let phases = design_phases(factor);
        let gain = phases.iter().map(|p| p.iter().map(|v| v.abs()).sum::<f32>()).fold(1.0f32, f32::max) * 1.0001;
        TruePeakMeter {
            channels,
            phases,
            gain,
            state: (0..channels)
                .map(|_| ChannelState {
                    buf: [0.0; 2 * TAPS],
                    pos: 0,
                    max: 0.0,
                    prev_block_max: 0.0,
                    last_over: None,
                    frames: 0,
                    over_count: 0,
                    over_events: SpreadTimes::new(200),
                    over_event_total: 0,
                })
                .collect(),
            sample_rate,
            max_events: 200,
        }
    }

    pub fn factor(&self) -> usize {
        self.phases.len() + 1
    }

    #[inline]
    fn push_compute(st: &mut ChannelState, phases: &[[f32; TAPS]], x: f32, sample_rate: u32) {
        st.buf[st.pos] = x;
        st.buf[st.pos + TAPS] = x;
        let window: &[f32; TAPS] = st.buf[st.pos + 1..st.pos + 1 + TAPS].try_into().expect("window");
        // The zero phase is the delayed input sample itself.
        let mut peak = window[TAPS / 2 - 1].abs();
        for ph in phases {
            let (mut a0, mut a1, mut a2, mut a3) = (0f32, 0f32, 0f32, 0f32);
            for i in (0..TAPS).step_by(4) {
                a0 += window[i] * ph[i];
                a1 += window[i + 1] * ph[i + 1];
                a2 += window[i + 2] * ph[i + 2];
                a3 += window[i + 3] * ph[i + 3];
            }
            peak = peak.max(((a0 + a1) + (a2 + a3)).abs());
        }
        st.pos = (st.pos + 1) % TAPS;
        if peak > st.max {
            st.max = peak;
        }
        if peak > 1.0 {
            st.over_count += 1;
            let gap = (sample_rate as u64 / 100).max(1);
            let is_new = st.last_over.is_none_or(|last| st.frames.saturating_sub(last) > gap);
            if is_new {
                st.over_event_total += 1;
                // The interpolator delays its output by half its length.
                let t = (st.frames as f64 - (TAPS / 2) as f64).max(0.0) / sample_rate as f64;
                st.over_events.push(t, 0);
            }
            st.last_over = Some(st.frames);
        }
        st.frames += 1;
    }

    #[inline]
    fn push_skip(st: &mut ChannelState, x: f32) {
        st.buf[st.pos] = x;
        st.buf[st.pos + TAPS] = x;
        st.pos = (st.pos + 1) % TAPS;
        st.frames += 1;
    }

    /// Processes interleaved frames.
    pub fn process(&mut self, interleaved: &[f32]) {
        let ch = self.channels;
        if ch == 0 {
            return;
        }
        let frames = interleaved.len() / ch;
        let mut block = [0f32; BLOCK];
        for c in 0..ch {
            let st = &mut self.state[c];
            let mut f = 0;
            while f < frames {
                let len = BLOCK.min(frames - f);
                let mut bmax = 0f32;
                for (i, slot) in block.iter_mut().enumerate().take(len) {
                    let x = interleaved[(f + i) * ch + c];
                    *slot = x;
                    bmax = bmax.max(x.abs());
                }
                let bound = bmax.max(st.prev_block_max) * self.gain;
                if bound > st.max.min(1.0) || bound > 1.0 {
                    for &x in &block[..len] {
                        Self::push_compute(st, &self.phases, x, self.sample_rate);
                    }
                } else {
                    for &x in &block[..len] {
                        Self::push_skip(st, x);
                    }
                }
                st.prev_block_max = bmax;
                f += len;
            }
        }
    }

    /// Flushes the interpolator with silence so the final samples are evaluated.
    pub fn finish(&mut self) {
        let silence = vec![0f32; self.channels * TAPS];
        for st in &mut self.state {
            // Force evaluation of the flush (the tail can hold the peak).
            st.prev_block_max = f32::INFINITY;
        }
        self.process(&silence);
    }

    /// Per-channel true peak (linear).
    pub fn channel_peaks(&self) -> Vec<f32> {
        self.state.iter().map(|s| s.max).collect()
    }

    pub fn peak(&self) -> f32 {
        self.state.iter().map(|s| s.max).fold(0.0, f32::max)
    }

    /// Interpolated values above 0 dBFS, summed over channels.
    pub fn over_count(&self) -> u64 {
        self.state.iter().map(|s| s.over_count).sum()
    }

    /// Start times (seconds) of over events on any channel, merged.
    pub fn over_events(&self) -> Vec<f64> {
        let mut v: Vec<f64> = self.state.iter().flat_map(|s| s.over_events.items().iter().map(|&(t, _)| t)).collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        v.dedup_by(|a, b| (*a - *b).abs() < 0.01);
        thin_evenly(v, self.max_events)
    }

    /// Number of over events (the busiest channel's count).
    pub fn over_event_total(&self) -> u64 {
        self.state.iter().map(|s| s.over_event_total).max().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    /// Sine with 50 ms raised-cosine fades: an abrupt start or stop is a
    /// step, and a band-limited reconstruction of a step genuinely overshoots.
    fn tp_of_sine(fs: u32, f: f64, amp: f64, phase: f64) -> f64 {
        let n = (fs as f64 * 0.5) as usize;
        let fade = (fs as f64 * 0.05) as usize;
        let x: Vec<f32> = (0..n)
            .map(|i| {
                let g = if i < fade {
                    0.5 - 0.5 * (PI * i as f64 / fade as f64).cos()
                } else if i >= n - fade {
                    0.5 - 0.5 * (PI * (n - 1 - i) as f64 / fade as f64).cos()
                } else {
                    1.0
                };
                (g * amp * (2.0 * PI * f * i as f64 / fs as f64 + phase).sin()) as f32
            })
            .collect();
        let mut m = TruePeakMeter::new(fs, 1);
        m.process(&x);
        m.finish();
        m.peak() as f64
    }

    #[test]
    fn quarter_rate_sine_at_45_degrees() {
        // Samples land at ±0.707·A; the true peak is A (+3 dB above sample peak).
        let tp = tp_of_sine(44_100, 11_025.0, 0.5, PI / 4.0);
        assert!((20.0 * (tp / 0.5).log10()).abs() < 0.05, "tp {tp}");
    }

    #[test]
    fn accurate_across_band() {
        for fs in [44_100u32, 48_000, 96_000] {
            for f in [100.0, 1_000.0, 5_000.0, 10_000.0, 15_000.0, 17_500.0] {
                for phase in [0.1, 0.7, 1.3] {
                    let tp = tp_of_sine(fs, f, 0.5, phase);
                    let err = 20.0 * (tp / 0.5).log10();
                    assert!(err.abs() < 0.15, "fs {fs} f {f} phase {phase}: {err:.3} dB");
                }
            }
        }
    }

    #[test]
    fn detects_overs() {
        let fs = 48_000;
        let x: Vec<f32> = (0..fs as usize).map(|i| (1.1 * (2.0 * PI * 997.0 * i as f64 / fs as f64).sin()) as f32).collect();
        let mut m = TruePeakMeter::new(fs, 1);
        m.process(&x);
        m.finish();
        assert!(m.over_count() > 1000);
        // Overs on every cycle form one continuous event (gaps < 10 ms).
        assert_eq!(m.over_event_total(), 1);
        assert!((m.peak() - 1.1).abs() < 0.01);
    }
}
