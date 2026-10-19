//! Waveform overview: per-bucket min/max/RMS of the mono mix plus the RMS of
//! three frequency bands (low < 200 Hz, mid 200 Hz–2.5 kHz, high > 2.5 kHz),
//! which the interface uses to colour the waveform by spectral content.
//!
//! The bucket size is fixed up front when the length is known; otherwise
//! buckets are merged pairwise whenever the count doubles, so memory stays
//! bounded for streams of unknown length.

use crate::dsp::biquad::{BiquadCoef, BiquadState};

pub const WAVEFORM_BUCKETS: usize = 1200;

#[derive(Clone, Copy, Debug, Default)]
pub struct Bucket {
    pub min: f32,
    pub max: f32,
    pub sum_sq: f64,
    pub low_sq: f64,
    pub mid_sq: f64,
    pub high_sq: f64,
    pub frames: u32,
}

impl Bucket {
    fn merge(a: &Bucket, b: &Bucket) -> Bucket {
        Bucket {
            min: a.min.min(b.min),
            max: a.max.max(b.max),
            sum_sq: a.sum_sq + b.sum_sq,
            low_sq: a.low_sq + b.low_sq,
            mid_sq: a.mid_sq + b.mid_sq,
            high_sq: a.high_sq + b.high_sq,
            frames: a.frames + b.frames,
        }
    }

    pub fn rms(&self) -> f32 {
        if self.frames == 0 {
            0.0
        } else {
            (self.sum_sq / self.frames as f64).sqrt() as f32
        }
    }

    pub fn band_rms(&self) -> (f32, f32, f32) {
        let n = self.frames.max(1) as f64;
        ((self.low_sq / n).sqrt() as f32, (self.mid_sq / n).sqrt() as f32, (self.high_sq / n).sqrt() as f32)
    }
}

#[derive(Clone, Debug, Default)]
pub struct Waveform {
    pub buckets: Vec<Bucket>,
    pub secs_per_bucket: f64,
}

pub struct WaveformBuilder {
    channels: usize,
    sample_rate: u32,
    per_bucket: u64,
    cur: Bucket,
    buckets: Vec<Bucket>,
    lp: BiquadCoef,
    hp: BiquadCoef,
    s_lp: BiquadState,
    s_hp: BiquadState,
}

impl WaveformBuilder {
    pub fn new(sample_rate: u32, channels: usize, frames_hint: Option<u64>) -> Self {
        let fs = sample_rate as f64;
        let q = std::f64::consts::FRAC_1_SQRT_2;
        let per_bucket = frames_hint.map(|f| f.div_ceil(WAVEFORM_BUCKETS as u64).max(1)).unwrap_or(((fs * 0.05) as u64).max(1));
        WaveformBuilder {
            channels,
            sample_rate,
            per_bucket,
            cur: Bucket { min: f32::MAX, max: f32::MIN, ..Default::default() },
            buckets: Vec::new(),
            lp: BiquadCoef::lowpass(fs, 200.0, q),
            hp: BiquadCoef::highpass(fs, 2500.0, q),
            s_lp: Default::default(),
            s_hp: Default::default(),
        }
    }

    pub fn process(&mut self, interleaved: &[f32]) {
        let ch = self.channels;
        if ch == 0 {
            return;
        }
        let inv = 1.0 / ch as f32;
        for frame in interleaved.chunks_exact(ch) {
            let m = frame.iter().sum::<f32>() * inv;
            let md = m as f64;
            let low = self.s_lp.process(&self.lp, md);
            let high = self.s_hp.process(&self.hp, md);
            // Mid band: what the low and high bands leave (a complementary
            // split; exact phase alignment is irrelevant for colouring).
            let mid = md - low - high;
            let b = &mut self.cur;
            b.min = b.min.min(m);
            b.max = b.max.max(m);
            b.sum_sq += md * md;
            b.low_sq += low * low;
            b.mid_sq += mid * mid;
            b.high_sq += high * high;
            b.frames += 1;
            if b.frames as u64 == self.per_bucket {
                self.push();
            }
        }
    }

    fn push(&mut self) {
        let b = std::mem::replace(&mut self.cur, Bucket { min: f32::MAX, max: f32::MIN, ..Default::default() });
        self.buckets.push(b);
        if self.buckets.len() >= 2 * WAVEFORM_BUCKETS {
            self.merge();
        }
    }

    fn merge(&mut self) {
        self.buckets = self.buckets.chunks(2).map(|p| if p.len() == 2 { Bucket::merge(&p[0], &p[1]) } else { p[0] }).collect();
        self.per_bucket *= 2;
    }

    pub fn finish(mut self) -> Waveform {
        if self.cur.frames > 0 {
            let b = self.cur;
            self.buckets.push(b);
        }
        while self.buckets.len() > WAVEFORM_BUCKETS {
            self.merge();
        }
        for b in &mut self.buckets {
            if b.frames == 0 {
                b.min = 0.0;
                b.max = 0.0;
            }
        }
        Waveform { secs_per_bucket: self.per_bucket as f64 / self.sample_rate as f64, buckets: self.buckets }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    #[test]
    fn unknown_length_stays_bounded_and_bands_separate() {
        let fs = 44_100u32;
        // 10 s of 60 Hz then 10 s of 8 kHz.
        let mut x = Vec::new();
        for i in 0..fs as usize * 20 {
            let t = i as f64 / fs as f64;
            let f = if t < 10.0 { 60.0 } else { 8000.0 };
            x.push((0.5 * (2.0 * PI * f * t).sin()) as f32);
        }
        let mut w = WaveformBuilder::new(fs, 1, None);
        for c in x.chunks(5000) {
            w.process(c);
        }
        let wf = w.finish();
        assert!(wf.buckets.len() <= WAVEFORM_BUCKETS && wf.buckets.len() > 100);
        let first = wf.buckets[10].band_rms();
        let last = wf.buckets[wf.buckets.len() - 10].band_rms();
        assert!(first.0 > 10.0 * first.2, "low dominates at the start: {first:?}");
        assert!(last.2 > 10.0 * last.0, "high dominates at the end: {last:?}");
        assert!((wf.buckets[10].max - 0.5).abs() < 0.01);
    }

    #[test]
    fn known_length_uses_requested_buckets() {
        let fs = 48_000u32;
        let n = fs as u64 * 30;
        let mut w = WaveformBuilder::new(fs, 2, Some(n));
        w.process(&vec![0.1f32; n as usize * 2]);
        let wf = w.finish();
        assert_eq!(wf.buckets.len(), WAVEFORM_BUCKETS);
        assert!((wf.secs_per_bucket - 0.025).abs() < 1e-9);
    }
}
