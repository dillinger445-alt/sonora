//! Sample-level statistics: peak, RMS, DC offset, decoder overs and hard
//! clipping.
//!
//! Hard clipping is detected as *flat runs*: three or more consecutive
//! samples with exactly the same value at the channel's own ceiling (its
//! maximum absolute level). Measuring at the ceiling rather than at 0 dBFS
//! also catches masters that were clipped and then normalised to e.g.
//! -0.3 dBFS. Lossy decoders smear flat tops into ringing, so for lossy
//! sources flat-run counts are not meaningful; samples decoded above full
//! scale ("overs") are reported instead.

use crate::util::{amp_to_db, SpreadTimes};

/// Level bins: 0.01 dB steps from -6 dBFS to +6 dBFS.
const BIN_DB: f64 = 0.01;
const MIN_DB: f64 = -6.0;
const NUM_BINS: usize = 1200;
const MAX_LEN_BUCKET: usize = 64;
/// Run-level tolerance below the ceiling still counted as "at the ceiling".
const CEILING_TOLERANCE_DB: f64 = 0.03;
/// Runs must be at least this long to count as clipping.
pub const MIN_CLIP_RUN: usize = 3;
const MAX_EVENT_TIMES: usize = 400;

fn level_bin(amp: f32) -> Option<usize> {
    let db = amp_to_db(amp as f64);
    if db < MIN_DB {
        return None;
    }
    Some((((db - MIN_DB) / BIN_DB) as usize).min(NUM_BINS - 1))
}

#[derive(Clone, Debug, Default)]
pub struct ChannelLevels {
    pub peak: f32,
    pub peak_frame: u64,
    pub sum: f64,
    pub sum_sq: f64,
    pub count: u64,
    /// Samples with |x| > 1.0 (only possible for float or lossy sources).
    pub overs: u64,
    pub max_over: f32,
}

impl ChannelLevels {
    pub fn rms(&self) -> f64 {
        if self.count == 0 {
            0.0
        } else {
            (self.sum_sq / self.count as f64).sqrt()
        }
    }

    pub fn dc(&self) -> f64 {
        if self.count == 0 {
            0.0
        } else {
            self.sum / self.count as f64
        }
    }
}

struct RunState {
    value: f32,
    len: usize,
    start_frame: u64,
}

struct ChannelClip {
    /// run_hist[bin * MAX_LEN_BUCKET + min(len, MAX_LEN_BUCKET-1)] = number of runs.
    run_hist: Vec<u32>,
    /// Total samples in runs per bin (exact, not bucketed).
    run_samples: Vec<u64>,
    longest: Vec<u32>,
    run: RunState,
    /// Event times (tagged with their level bin) of long runs near the running maximum.
    events: SpreadTimes,
    running_max_bin: usize,
}

impl ChannelClip {
    fn new() -> Self {
        ChannelClip {
            run_hist: vec![0; NUM_BINS * MAX_LEN_BUCKET],
            run_samples: vec![0; NUM_BINS],
            longest: vec![0; NUM_BINS],
            run: RunState { value: f32::NAN, len: 0, start_frame: 0 },
            events: SpreadTimes::new(MAX_EVENT_TIMES),
            running_max_bin: 0,
        }
    }

    fn close_run(&mut self, sample_rate: u32) {
        let len = self.run.len;
        if len >= 2 {
            if let Some(bin) = level_bin(self.run.value.abs()) {
                self.run_hist[bin * MAX_LEN_BUCKET + len.min(MAX_LEN_BUCKET - 1)] += 1;
                self.run_samples[bin] += len as u64;
                self.longest[bin] = self.longest[bin].max(len as u32);
                if len >= MIN_CLIP_RUN {
                    let tol_bins = (CEILING_TOLERANCE_DB / BIN_DB).ceil() as usize + 1;
                    if bin + tol_bins >= self.running_max_bin {
                        if bin > self.running_max_bin {
                            self.running_max_bin = bin;
                            self.events.retain(|&(_, b)| b as usize + tol_bins >= bin);
                        }
                        self.events.push(self.run.start_frame as f64 / sample_rate as f64, bin as u32);
                    }
                }
            }
        }
        self.run.len = 0;
        self.run.value = f32::NAN;
    }
}

/// Clipping statistics for one channel or the whole file.
#[derive(Clone, Debug, Default)]
pub struct ClipStats {
    /// Level of the channel ceiling (dBFS).
    pub ceiling_db: f64,
    /// Number of flat runs of at least [`MIN_CLIP_RUN`] samples at the ceiling.
    pub events: u64,
    /// Samples inside those runs.
    pub samples: u64,
    pub longest_run: u32,
    /// Runs of exactly two equal samples at the ceiling (not counted as clipping).
    pub pairs: u64,
    pub event_times: Vec<f64>,
}

pub struct LevelMeter {
    channels: usize,
    sample_rate: u32,
    pub levels: Vec<ChannelLevels>,
    clip: Vec<ChannelClip>,
    detect_runs: bool,
    frame: u64,
}

impl LevelMeter {
    /// `detect_runs` should be false for lossy sources.
    pub fn new(sample_rate: u32, channels: usize, detect_runs: bool) -> Self {
        LevelMeter {
            channels,
            sample_rate,
            levels: vec![ChannelLevels::default(); channels],
            clip: if detect_runs { (0..channels).map(|_| ChannelClip::new()).collect() } else { Vec::new() },
            detect_runs,
            frame: 0,
        }
    }

    pub fn process(&mut self, interleaved: &[f32]) {
        let ch = self.channels;
        if ch == 0 {
            return;
        }
        for (c, lv) in self.levels.iter_mut().enumerate() {
            let mut sum = 0f64;
            let mut sum_sq = 0f64;
            let mut peak = lv.peak;
            let mut peak_frame = lv.peak_frame;
            let mut overs = 0u64;
            let mut max_over = lv.max_over;
            let mut n = 0u64;
            for (i, &x) in interleaved.iter().skip(c).step_by(ch).enumerate() {
                let xd = x as f64;
                sum += xd;
                sum_sq += xd * xd;
                let a = x.abs();
                if a > peak {
                    peak = a;
                    peak_frame = self.frame + i as u64;
                }
                if a > 1.0 {
                    overs += 1;
                    max_over = max_over.max(a);
                }
                n += 1;
            }
            lv.sum += sum;
            lv.sum_sq += sum_sq;
            lv.peak = peak;
            lv.peak_frame = peak_frame;
            lv.overs += overs;
            lv.max_over = max_over;
            lv.count += n;
        }
        if self.detect_runs {
            let sr = self.sample_rate;
            for (c, st) in self.clip.iter_mut().enumerate() {
                for (i, &x) in interleaved.iter().skip(c).step_by(ch).enumerate() {
                    // Only samples above -6 dBFS can be part of a clip run.
                    if x.abs() >= 0.5 && x == st.run.value {
                        st.run.len += 1;
                    } else {
                        if st.run.len > 0 {
                            st.close_run(sr);
                        }
                        if x.abs() >= 0.5 {
                            st.run.value = x;
                            st.run.len = 1;
                            st.run.start_frame = self.frame + i as u64;
                        }
                    }
                }
            }
        }
        self.frame += (interleaved.len() / ch) as u64;
    }

    pub fn frames(&self) -> u64 {
        self.frame
    }

    /// Finalises run tracking and returns per-channel clip statistics.
    pub fn clip_stats(&mut self) -> Option<Vec<ClipStats>> {
        if !self.detect_runs {
            return None;
        }
        let sr = self.sample_rate;
        let mut out = Vec::with_capacity(self.channels);
        for (c, st) in self.clip.iter_mut().enumerate() {
            if st.run.len > 0 {
                st.close_run(sr);
            }
            let peak = self.levels[c].peak;
            let mut stats = ClipStats { ceiling_db: amp_to_db(peak as f64), ..Default::default() };
            if let Some(top) = level_bin(peak) {
                let tol_bins = (CEILING_TOLERANCE_DB / BIN_DB).ceil() as usize;
                let lo = top.saturating_sub(tol_bins);
                for bin in lo..=top {
                    let row = &st.run_hist[bin * MAX_LEN_BUCKET..(bin + 1) * MAX_LEN_BUCKET];
                    stats.pairs += row[2] as u64;
                    let runs: u64 = row[MIN_CLIP_RUN..].iter().map(|&v| v as u64).sum();
                    stats.events += runs;
                    if runs > 0 {
                        stats.longest_run = stats.longest_run.max(st.longest[bin]);
                    }
                    // run_samples includes pairs; subtract them.
                    stats.samples += st.run_samples[bin].saturating_sub(2 * row[2] as u64);
                }
                stats.event_times = st.events.items().iter().filter(|&&(_, b)| b as usize >= lo).map(|&(t, _)| t).collect();
            }
            out.push(stats);
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    fn sine(fs: u32, f: f64, amp: f64, secs: f64) -> Vec<f32> {
        (0..(fs as f64 * secs) as usize).map(|i| (amp * (2.0 * PI * f * i as f64 / fs as f64).sin()) as f32).collect()
    }

    fn quantize16(x: &[f32]) -> Vec<f32> {
        x.iter().map(|&v| ((v as f64 * 32768.0).round().clamp(-32768.0, 32767.0) / 32768.0) as f32).collect()
    }

    #[test]
    fn clean_sine_has_no_clipping() {
        let x = quantize16(&sine(44_100, 997.0, 0.9, 2.0));
        let mut m = LevelMeter::new(44_100, 1, true);
        m.process(&x);
        let s = m.clip_stats().unwrap();
        assert_eq!(s[0].events, 0);
        assert!((m.levels[0].peak - 0.9).abs() < 1e-3);
        assert!((m.levels[0].rms() - 0.9 / 2f64.sqrt()).abs() < 1e-3);
    }

    #[test]
    fn hard_clipped_sine_is_detected_below_full_scale() {
        // Clip at 0.7 then "normalise" to -0.5 dBFS: ceiling is below 0 dBFS.
        let gain = 10f64.powf(-0.5 / 20.0) / 0.7;
        let x: Vec<f32> = sine(44_100, 220.0, 1.0, 1.0).iter().map(|&v| (v.clamp(-0.7, 0.7) as f64 * gain) as f32).collect();
        let x = quantize16(&x);
        let mut m = LevelMeter::new(44_100, 1, true);
        m.process(&x);
        let s = &m.clip_stats().unwrap()[0];
        // 220 Hz for 1 s: 440 clipped half-cycles.
        assert!(s.events >= 430 && s.events <= 445, "events {}", s.events);
        assert!((s.ceiling_db + 0.5).abs() < 0.01);
        assert!(s.longest_run > 20);
        assert!(!s.event_times.is_empty());
    }

    #[test]
    fn overs_and_dc() {
        let x: Vec<f32> = sine(48_000, 100.0, 1.2, 0.5).iter().map(|v| v + 0.05).collect();
        let mut m = LevelMeter::new(48_000, 1, false);
        m.process(&x);
        assert!(m.levels[0].overs > 0);
        assert!((m.levels[0].dc() - 0.05).abs() < 1e-3);
        assert!(m.clip_stats().is_none());
    }

    #[test]
    fn stereo_interleaving() {
        let l = sine(44_100, 440.0, 0.5, 0.2);
        let mut x = Vec::new();
        for v in &l {
            x.push(*v);
            x.push(v * 0.25);
        }
        let mut m = LevelMeter::new(44_100, 2, true);
        for chunk in x.chunks(1000) {
            m.process(chunk);
        }
        assert!((m.levels[0].peak - 0.5).abs() < 1e-3);
        assert!((m.levels[1].peak - 0.125).abs() < 1e-3);
        assert_eq!(m.frames(), l.len() as u64);
    }
}
