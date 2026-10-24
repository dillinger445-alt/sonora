//! Silence, digital-silence gaps ("dropouts") and abrupt endings.
//!
//! - Silence: 10 ms blocks whose peak is below -60 dBFS.
//! - Dropout: a run of exact digital zero on every channel lasting at least
//!   10 ms, surrounded on both sides by audio louder than -45 dBFS RMS. Music
//!   practically never jumps from a loud passage to exact digital zero and
//!   back, so these gaps are strong evidence of a damaged rip, transfer or edit.
//! - Abrupt cut-off: the file ends in at least one second of digital zero that
//!   begins abruptly while audio was loud — a typical signature of a partially
//!   downloaded file whose remainder was zero-filled.

const SILENCE_PEAK: f32 = 0.001; // -60 dBFS
const LOUD_DB: f64 = -45.0;
const MAX_REPORTED: usize = 200;

#[derive(Clone, Debug, PartialEq)]
pub struct Dropout {
    pub start: f64,
    pub duration: f64,
    pub pre_db: f64,
    pub post_db: f64,
}

#[derive(Clone, Debug, Default)]
pub struct SilenceResult {
    pub duration: f64,
    pub leading_silence: f64,
    pub trailing_silence: f64,
    /// Fraction of 10 ms blocks below -60 dBFS.
    pub silent_fraction: f64,
    /// Entire file is below -60 dBFS.
    pub all_silent: bool,
    /// Entire file is exact digital zero.
    pub all_zero: bool,
    pub dropouts: Vec<Dropout>,
    pub dropout_count: u64,
    /// The file ends with this many seconds of digital zero that began abruptly.
    pub abrupt_cutoff: Option<(f64, f64)>, // (start time, length)
    /// Level (dBFS RMS) of the last 50 ms of non-zero audio.
    pub ending_level_db: f64,
    /// Silent regions longer than 1 s (start, end) for display, at most 50.
    pub silent_regions: Vec<(f64, f64)>,
}

struct Pending {
    start_frame: u64,
    len: u64,
    pre_db: f64,
    post_acc: f64,
    post_n: u64,
}

pub struct SilenceMeter {
    sample_rate: u32,
    channels: usize,
    frame: u64,
    block_len: u64,
    block_pos: u64,
    block_max: f32,
    blocks: u64,
    silent_blocks: u64,
    first_loud_block: Option<u64>,
    last_loud_block: Option<u64>,
    ema: f64,
    alpha: f64,
    zero_run_start: Option<u64>,
    zero_run_pre_db: f64,
    any_nonzero: bool,
    pending: Option<Pending>,
    dropouts: Vec<Dropout>,
    dropout_count: u64,
    last_loud_ema: f64,
    silent_run_start_block: Option<u64>,
    silent_regions: Vec<(f64, f64)>,
}

impl SilenceMeter {
    pub fn new(sample_rate: u32, channels: usize) -> Self {
        let fs = sample_rate as f64;
        SilenceMeter {
            sample_rate,
            channels,
            frame: 0,
            block_len: ((fs * 0.01) as u64).max(1),
            block_pos: 0,
            block_max: 0.0,
            blocks: 0,
            silent_blocks: 0,
            first_loud_block: None,
            last_loud_block: None,
            ema: 0.0,
            // ~30 ms time constant.
            alpha: 1.0 - (-1.0 / (0.03 * fs)).exp(),
            zero_run_start: None,
            zero_run_pre_db: -200.0,
            any_nonzero: false,
            pending: None,
            dropouts: Vec::new(),
            dropout_count: 0,
            last_loud_ema: 0.0,
            silent_run_start_block: None,
            silent_regions: Vec::new(),
        }
    }

    fn db(ms: f64) -> f64 {
        10.0 * ms.max(1e-20).log10()
    }

    fn post_window(&self) -> u64 {
        (self.sample_rate as u64 / 20).max(1) // 50 ms
    }

    fn resolve_pending(&mut self, p: Pending) {
        let post_db = if p.post_n > 0 { Self::db(p.post_acc / p.post_n as f64) } else { -200.0 };
        if p.pre_db > LOUD_DB && post_db > LOUD_DB {
            self.dropout_count += 1;
            if self.dropouts.len() < MAX_REPORTED {
                let fs = self.sample_rate as f64;
                self.dropouts.push(Dropout { start: p.start_frame as f64 / fs, duration: p.len as f64 / fs, pre_db: p.pre_db, post_db });
            }
        }
    }

    pub fn process(&mut self, interleaved: &[f32]) {
        let ch = self.channels;
        if ch == 0 {
            return;
        }
        let min_gap = (self.sample_rate as u64 / 100).max(1); // 10 ms
        let post_window = self.post_window();
        for frame in interleaved.chunks_exact(ch) {
            let mut peak = 0f32;
            let mut sq = 0f64;
            let mut zero = true;
            for &x in frame {
                peak = peak.max(x.abs());
                sq += (x as f64) * (x as f64);
                if x != 0.0 {
                    zero = false;
                }
            }
            sq /= ch as f64;

            // Post-gap level measurement for a pending dropout candidate.
            if let Some(p) = self.pending.as_mut() {
                p.post_acc += sq;
                p.post_n += 1;
                if p.post_n >= post_window {
                    let p = self.pending.take().unwrap();
                    self.resolve_pending(p);
                }
            }

            if zero {
                if self.zero_run_start.is_none() {
                    self.zero_run_start = Some(self.frame);
                    self.zero_run_pre_db = Self::db(self.ema);
                }
            } else {
                if let Some(start) = self.zero_run_start.take() {
                    let len = self.frame - start;
                    if self.any_nonzero && len >= min_gap {
                        if let Some(prev) = self.pending.take() {
                            self.resolve_pending(prev);
                        }
                        self.pending = Some(Pending { start_frame: start, len, pre_db: self.zero_run_pre_db, post_acc: 0.0, post_n: 0 });
                    }
                }
                self.any_nonzero = true;
                self.ema += self.alpha * (sq - self.ema);
                self.last_loud_ema = self.ema;
            }

            self.block_max = self.block_max.max(peak);
            self.block_pos += 1;
            if self.block_pos == self.block_len {
                self.end_block();
            }
            self.frame += 1;
        }
    }

    /// Judges the block just ended as silent or loud.
    fn end_block(&mut self) {
        let silent = self.block_max < SILENCE_PEAK;
        if silent {
            self.silent_blocks += 1;
            if self.silent_run_start_block.is_none() {
                self.silent_run_start_block = Some(self.blocks);
            }
        } else {
            if self.first_loud_block.is_none() {
                self.first_loud_block = Some(self.blocks);
            }
            self.last_loud_block = Some(self.blocks);
            self.close_silent_region(self.blocks);
        }
        self.blocks += 1;
        self.block_pos = 0;
        self.block_max = 0.0;
    }

    fn close_silent_region(&mut self, end_block: u64) {
        if let Some(start) = self.silent_run_start_block.take() {
            let secs = (end_block - start) as f64 * 0.01;
            if secs >= 1.0 && self.silent_regions.len() < 50 {
                self.silent_regions.push((start as f64 * 0.01, end_block as f64 * 0.01));
            }
        }
    }

    pub fn finish(&mut self) -> SilenceResult {
        let fs = self.sample_rate as f64;
        if let Some(p) = self.pending.take() {
            self.resolve_pending(p);
        }
        // The last, partial block counts too: a sound shorter than one
        // block is not silence (B78).
        if self.block_pos > 0 {
            self.end_block();
        }
        let blocks = self.blocks;
        self.close_silent_region(blocks);
        let duration = self.frame as f64 / fs;
        let block_secs = self.block_len as f64 / fs;
        let all_silent = self.first_loud_block.is_none();
        let leading = match self.first_loud_block {
            Some(b) => b as f64 * block_secs,
            None => duration,
        };
        let trailing = match self.last_loud_block {
            Some(b) => (duration - (b + 1) as f64 * block_secs).max(0.0),
            None => duration,
        };
        let mut abrupt = None;
        if let Some(start) = self.zero_run_start {
            let len = (self.frame - start) as f64 / fs;
            if self.any_nonzero && len >= 1.0 && self.zero_run_pre_db > -30.0 {
                abrupt = Some((start as f64 / fs, len));
            }
        }
        SilenceResult {
            duration,
            leading_silence: leading,
            trailing_silence: trailing,
            silent_fraction: if blocks > 0 { self.silent_blocks as f64 / blocks as f64 } else { 1.0 },
            all_silent,
            all_zero: !self.any_nonzero,
            dropouts: std::mem::take(&mut self.dropouts),
            dropout_count: self.dropout_count,
            abrupt_cutoff: abrupt,
            ending_level_db: if self.any_nonzero { Self::db(self.last_loud_ema) } else { -200.0 },
            silent_regions: std::mem::take(&mut self.silent_regions),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    fn tone(fs: u32, secs: f64, amp: f64) -> Vec<f32> {
        (0..(fs as f64 * secs) as usize).map(|i| (amp * (2.0 * PI * 440.0 * i as f64 / fs as f64).sin()) as f32).collect()
    }

    #[test]
    fn leading_trailing_and_dropout() {
        let fs = 44_100;
        let mut x = vec![0f32; fs as usize]; // 1 s leading digital silence
        x.extend(tone(fs, 2.0, 0.5));
        x.extend(vec![0f32; (fs as f64 * 0.2) as usize]); // 200 ms dropout
        x.extend(tone(fs, 2.0, 0.5));
        x.extend(vec![0.0001f32; (fs as f64 * 1.5) as usize]); // quiet tail (not zero)
        let mut m = SilenceMeter::new(fs, 1);
        for c in x.chunks(3000) {
            m.process(c);
        }
        let r = m.finish();
        assert!((r.leading_silence - 1.0).abs() < 0.02, "{}", r.leading_silence);
        assert!((r.trailing_silence - 1.5).abs() < 0.02, "{}", r.trailing_silence);
        assert_eq!(r.dropout_count, 1);
        assert!((r.dropouts[0].start - 3.0).abs() < 0.01);
        assert!((r.dropouts[0].duration - 0.2).abs() < 0.01);
        assert!(r.abrupt_cutoff.is_none());
        assert!(!r.all_silent);
    }

    #[test]
    fn zero_filled_tail_is_abrupt_cutoff() {
        let fs = 48_000;
        let mut x = tone(fs, 3.0, 0.5);
        x.extend(vec![0f32; fs as usize * 4]);
        let mut m = SilenceMeter::new(fs, 1);
        m.process(&x);
        let r = m.finish();
        let (start, len) = r.abrupt_cutoff.unwrap();
        assert!((start - 3.0).abs() < 0.01);
        assert!((len - 4.0).abs() < 0.01);
        assert_eq!(r.dropout_count, 0);
    }

    #[test]
    fn fade_out_is_not_abrupt() {
        let fs = 48_000;
        let n = fs as usize * 3;
        let mut x: Vec<f32> = tone(fs, 3.0, 0.5).iter().enumerate().map(|(i, v)| v * (1.0 - i as f32 / n as f32).powi(4)).collect();
        x.extend(vec![0f32; fs as usize * 2]);
        let mut m = SilenceMeter::new(fs, 1);
        m.process(&x);
        assert!(m.finish().abrupt_cutoff.is_none());
    }

    #[test]
    fn all_zero() {
        let mut m = SilenceMeter::new(44_100, 2);
        m.process(&vec![0f32; 44_100 * 2]);
        let r = m.finish();
        assert!(r.all_silent && r.all_zero);
    }
}
