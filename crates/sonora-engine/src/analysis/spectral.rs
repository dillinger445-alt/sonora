//! Spectral analysis.
//!
//! The main spectrum is the *channel-average power* (|L|² + |R|²)/2, which,
//! unlike the mid signal (L+R)/2, cannot vanish when one channel is polarity
//! inverted. The side spectrum |L−R|²/4 is derived from the same complex
//! FFTs. One streaming pass with a Hann-windowed FFT (non-overlapping frames
//! of roughly 85–93 ms) collects:
//! - long-term average spectra (LTAS) of the channel average, the side
//!   signal and of quiet passages;
//! - per-frame energies of narrow high-frequency bands, kept (decimated if
//!   necessary) so that cutoff behaviour can be examined over time once the
//!   long-term cutoff is known;
//! - spectral-flatness and spectral-hole statistics per 1 kHz sub-band,
//!   measured on separate short frames (~23 ms) because lossy codecs zero
//!   coefficients per granule (13–26 ms) and long frames average the holes
//!   away. A "hole" is a bin more than 20 dB below its sub-band median: the
//!   median, unlike the mean, is not pulled up by strong tonal partials, so
//!   ordinary synth harmonics do not register as holes;
//! - a fixed-size spectrogram and a log-frequency display spectrum;
//! - the acoustic fingerprint, from a dedicated STFT whose frames have a
//!   fixed length *in seconds* and start on an exact 100 ms grid, so files
//!   at different sample rates produce comparable fingerprints.
//!
//! Power is normalised so a full-scale sine reads 0 dB in its peak bin.

use crate::dsp::fft::RealFft;
use crate::dsp::hann;

pub const SPECTROGRAM_ROWS: usize = 160;
pub const SPECTROGRAM_COLS: usize = 400;
pub const SPECTROGRAM_DB_MIN: f64 = -140.0;
pub const SPECTROGRAM_DB_MAX: f64 = -10.0;
const FP_BANDS: usize = 33;
const FP_LO_HZ: f64 = 300.0;
const FP_HI_HZ: f64 = 3000.0;
/// Fingerprint frames per second.
pub const FP_RATE: f64 = 10.0;
const HF_START_HZ: f64 = 1000.0;
const MAX_HF_BANDS: usize = 200;
const MAX_STORED_FRAMES: usize = 40_000;
const SFM_START_HZ: f64 = 3000.0;
const SFM_WIDTH_HZ: f64 = 1000.0;
const MAX_SFM_SUBBANDS: usize = 48;
const SFM_HIST_BINS: usize = 25;
/// Frames quieter than this (time-domain RMS of mid) are "inactive".
const ACTIVE_DB: f64 = -90.0;
/// Bins more than 20 dB below their sub-band median count as spectral holes.
const HOLE_RATIO: f32 = 0.01;
const LOG_SPECTRUM_POINTS: usize = 256;

/// FFT size giving ~10 Hz resolution and frames shorter than 100 ms.
pub fn fft_size_for(sample_rate: u32) -> usize {
    match sample_rate {
        0..=34_000 => 2048,
        34_001..=50_000 => 4096,
        50_001..=100_000 => 8192,
        100_001..=200_000 => 16384,
        _ => 32768,
    }
}

/// Encodes a dB value into a byte (0.667 dB steps, -160 … +10 dB).
#[inline]
pub fn db_to_q(db: f64) -> u8 {
    ((db + 160.0) * 1.5).round().clamp(0.0, 255.0) as u8
}

#[inline]
pub fn q_to_db(q: u8) -> f64 {
    q as f64 / 1.5 - 160.0
}

/// Natural logarithm accurate to ~1e-4, several times faster than `ln`;
/// used for spectral-flatness statistics only.
#[inline]
fn fast_ln(x: f32) -> f32 {
    let bits = x.max(1e-30).to_bits();
    let e = ((bits >> 23) & 0xff) as i32 - 127;
    let m = f32::from_bits((bits & 0x007f_ffff) | 0x3f80_0000);
    let t = m - 1.0;
    let ln_m = t * (0.999_495_6 + t * (-0.491_908_96 + t * (0.289_474_8 + t * (-0.136_062_75 + t * 0.032_158_45))));
    e as f32 * std::f32::consts::LN_2 + ln_m
}

#[inline]
fn pdb(p: f64) -> f64 {
    10.0 * p.max(1e-30).log10()
}

/// Per-frame high-frequency band energies (dB, byte-encoded).
#[derive(Clone, Debug, Default)]
pub struct HfFrames {
    pub start_hz: f64,
    pub band_hz: f64,
    pub bands: usize,
    /// Row-major `[frame][band]`.
    pub data: Vec<u8>,
    /// Reference energy of each stored frame (1–8 kHz), byte-encoded dB.
    pub reference: Vec<u8>,
    /// Start time (s) of each stored frame.
    pub times: Vec<f32>,
}

impl HfFrames {
    pub fn frames(&self) -> usize {
        self.times.len()
    }

    pub fn band_center(&self, b: usize) -> f64 {
        self.start_hz + (b as f64 + 0.5) * self.band_hz
    }

    pub fn band_of_hz(&self, hz: f64) -> Option<usize> {
        if hz < self.start_hz {
            return None;
        }
        let b = ((hz - self.start_hz) / self.band_hz) as usize;
        (b < self.bands).then_some(b)
    }

    pub fn row(&self, f: usize) -> &[u8] {
        &self.data[f * self.bands..(f + 1) * self.bands]
    }
}

#[derive(Clone, Debug, Default)]
pub struct SubbandStats {
    pub lo_hz: f64,
    pub hi_hz: f64,
    /// Frames in which the sub-band carried content.
    pub frames: u64,
    pub mean_sfm: f64,
    pub median_sfm: f64,
    /// Mean fraction of bins more than 20 dB below the sub-band median.
    pub mean_hole_ratio: f64,
}

#[derive(Clone, Debug, Default)]
pub struct Spectrogram {
    pub cols: usize,
    pub rows: usize,
    /// Column-major `[col][row]`, row 0 = 0 Hz; byte-encoded between
    /// [`SPECTROGRAM_DB_MIN`] and [`SPECTROGRAM_DB_MAX`].
    pub data: Vec<u8>,
    pub secs_per_col: f64,
    pub max_hz: f64,
}

#[derive(Clone, Debug)]
pub struct BandEnergy {
    pub id: &'static str,
    pub label: &'static str,
    pub lo_hz: f64,
    pub hi_hz: f64,
    /// Share of total energy (0–1).
    pub share: f64,
    /// Band energy relative to total energy (dB).
    pub rel_db: f64,
}

#[derive(Clone, Debug, Default)]
pub struct SpectralResult {
    pub fft_size: usize,
    pub bin_hz: f64,
    pub nyquist: f64,
    pub frame_secs: f64,
    pub frames: u64,
    pub active_frames: u64,
    /// Channel-average long-term spectrum (dB per bin).
    pub ltas_db: Vec<f32>,
    pub ltas_side_db: Option<Vec<f32>>,
    pub quiet_ltas_db: Option<Vec<f32>>,
    pub quiet_frames: u64,
    /// Mean RMS level (dBFS) of quiet frames (-100 … -40 dBFS).
    pub quiet_rms_db: Option<f64>,
    pub hf: HfFrames,
    pub subbands: Vec<SubbandStats>,
    pub spectrogram: Spectrogram,
    pub fingerprint: Vec<u32>,
    pub bands: Vec<BandEnergy>,
    /// Log-spaced display spectrum: (Hz, mid dB, side dB).
    pub display: Vec<(f32, f32, Option<f32>)>,
}

struct SubbandAcc {
    lo_bin: usize,
    hi_bin: usize,
    frames: u64,
    sfm_sum: f64,
    hole_sum: f64,
    hist: [u32; SFM_HIST_BINS],
}

pub struct SpectralAnalyzer {
    sample_rate: u32,
    channels: usize,
    n: usize,
    window: Vec<f32>,
    fft: RealFft,
    chan_a: Vec<f32>,
    chan_b: Vec<f32>,
    fill: usize,
    re_a: Vec<f32>,
    im_a: Vec<f32>,
    re_b: Vec<f32>,
    im_b: Vec<f32>,
    frame_index: u64,
    scratch_in: Vec<f32>,
    scratch_re: Vec<f32>,
    scratch_im: Vec<f32>,
    p_mid: Vec<f32>,
    p_side: Vec<f32>,
    norm: f32,
    ltas_mid: Vec<f64>,
    ltas_side: Vec<f64>,
    side_seen: bool,
    quiet_ltas: Vec<f64>,
    quiet_frames: u64,
    active_frames: u64,
    // High-frequency band storage.
    hf_band_hz: f64,
    hf_edges: Vec<usize>,
    hf_data: Vec<u8>,
    hf_ref: Vec<u8>,
    hf_times: Vec<f32>,
    hf_stride: u64,
    hf_seen: u64,
    ref_bins: (usize, usize),
    // Sub-band flatness/holes on short frames.
    short_n: usize,
    short_window: Vec<f32>,
    short_fft: RealFft,
    short_mid: Vec<f32>,
    short_side: Vec<f32>,
    short_fill: usize,
    short_in: Vec<f32>,
    p_short: Vec<f32>,
    short_norm: f32,
    sort_scratch: Vec<f32>,
    subbands: Vec<SubbandAcc>,
    quiet_ms_sum: f64,
    // Spectrogram.
    sg_row_edges: Vec<usize>,
    sg_acc: Vec<f64>,
    sg_frames: usize,
    sg_per_col: usize,
    sg_cols: Vec<Vec<f32>>,
    // Fingerprint STFT (fixed-duration frames on a 100 ms grid).
    fp_len: usize,
    fp_hop: usize,
    fp_window: Vec<f32>,
    fp_fft: RealFft,
    fp_mid: Vec<f32>,
    fp_side: Vec<f32>,
    fp_pos: usize,
    fp_in: Vec<f32>,
    fp_power: Vec<f32>,
    fp_edges: Vec<usize>,
    fp_prev: Option<[f32; FP_BANDS]>,
    fingerprint: Vec<u32>,
}

impl SpectralAnalyzer {
    pub fn new(sample_rate: u32, channels: usize, frames_hint: Option<u64>) -> Self {
        let n = fft_size_for(sample_rate);
        let bins = n / 2 + 1;
        let fs = sample_rate as f64;
        let bin_hz = fs / n as f64;
        let nyq = fs / 2.0;
        let window = hann(n);
        let wsum: f64 = window.iter().map(|&w| w as f64).sum();
        let norm = (4.0 / (wsum * wsum)) as f32;
        let to_bin = |hz: f64| ((hz / bin_hz).round() as usize).min(bins - 1);

        // HF bands from 1 kHz to Nyquist, at most MAX_HF_BANDS.
        let span = (nyq - HF_START_HZ).max(bin_hz);
        let mut band_hz = 250.0;
        while span / band_hz > MAX_HF_BANDS as f64 {
            band_hz *= 2.0;
        }
        let nb = (span / band_hz).floor().max(1.0) as usize;
        let hf_edges: Vec<usize> = (0..=nb).map(|b| to_bin(HF_START_HZ + b as f64 * band_hz)).collect();

        let short_n = (n / 4).max(256);
        let short_bin_hz = fs / short_n as f64;
        let short_window = hann(short_n);
        let short_wsum: f64 = short_window.iter().map(|&w| w as f64).sum();
        let to_short_bin = |hz: f64| ((hz / short_bin_hz).round() as usize).min(short_n / 2);
        let mut subbands = Vec::new();
        let mut lo = SFM_START_HZ;
        while lo + SFM_WIDTH_HZ <= nyq && subbands.len() < MAX_SFM_SUBBANDS {
            let (lb, hb) = (to_short_bin(lo), to_short_bin(lo + SFM_WIDTH_HZ));
            if hb > lb + 8 {
                subbands.push(SubbandAcc { lo_bin: lb, hi_bin: hb, frames: 0, sfm_sum: 0.0, hole_sum: 0.0, hist: [0; SFM_HIST_BINS] });
            }
            lo += SFM_WIDTH_HZ;
        }

        let sg_row_edges: Vec<usize> = (0..=SPECTROGRAM_ROWS).map(|r| (r * (bins - 1)) / SPECTROGRAM_ROWS).collect();
        let expected_frames = frames_hint.map(|f| (f / n as u64).max(1) as usize);
        let sg_per_col = expected_frames.map(|f| f.div_ceil(SPECTROGRAM_COLS).max(1)).unwrap_or(1);

        // ~92.9 ms frames (4096 samples at 44.1 kHz) every 100 ms, at any rate.
        let fp_len = ((fs * 4096.0 / 44_100.0).round() as usize).max(256);
        let fp_hop = (fs / FP_RATE).round() as usize;
        let fp_n = fp_len.next_power_of_two();
        let fp_bin_hz = fs / fp_n as f64;
        let ratio = (FP_HI_HZ / FP_LO_HZ).powf(1.0 / FP_BANDS as f64);
        let fp_edges: Vec<usize> = (0..=FP_BANDS).map(|i| ((FP_LO_HZ * ratio.powi(i as i32)) / fp_bin_hz).round() as usize).collect();

        SpectralAnalyzer {
            sample_rate,
            channels,
            n,
            window,
            fft: RealFft::new(n),
            chan_a: vec![0.0; n],
            chan_b: vec![0.0; n],
            fill: 0,
            re_a: vec![0.0; bins],
            im_a: vec![0.0; bins],
            re_b: vec![0.0; bins],
            im_b: vec![0.0; bins],
            frame_index: 0,
            scratch_in: vec![0.0; n],
            scratch_re: Vec::new(),
            scratch_im: Vec::new(),
            p_mid: vec![0.0; bins],
            p_side: vec![0.0; bins],
            norm,
            ltas_mid: vec![0.0; bins],
            ltas_side: vec![0.0; bins],
            side_seen: false,
            quiet_ltas: vec![0.0; bins],
            quiet_frames: 0,
            active_frames: 0,
            hf_band_hz: band_hz,
            hf_edges,
            hf_data: Vec::new(),
            hf_ref: Vec::new(),
            hf_times: Vec::new(),
            hf_stride: 1,
            hf_seen: 0,
            ref_bins: (to_bin(1000.0), to_bin(8000.0f64.min(nyq))),
            short_n,
            short_window,
            short_fft: RealFft::new(short_n),
            short_mid: vec![0.0; short_n],
            short_side: vec![0.0; short_n],
            short_fill: 0,
            short_in: vec![0.0; short_n],
            p_short: vec![0.0; short_n / 2 + 1],
            short_norm: (4.0 / (short_wsum * short_wsum)) as f32,
            sort_scratch: Vec::with_capacity(64),
            subbands,
            quiet_ms_sum: 0.0,
            sg_row_edges,
            sg_acc: vec![0.0; SPECTROGRAM_ROWS],
            sg_frames: 0,
            sg_per_col,
            sg_cols: Vec::new(),
            fp_len,
            fp_hop,
            fp_window: hann(fp_len),
            fp_fft: RealFft::new(fp_n),
            fp_mid: vec![0.0; fp_len],
            fp_side: vec![0.0; fp_len],
            fp_pos: 0,
            fp_in: vec![0.0; fp_n],
            fp_power: vec![0.0; fp_n / 2 + 1],
            fp_edges,
            fp_prev: None,
            fingerprint: Vec::new(),
        }
    }

    pub fn fft_size(&self) -> usize {
        self.n
    }

    pub fn process(&mut self, interleaved: &[f32]) {
        let ch = self.channels;
        if ch == 0 {
            return;
        }
        for frame in interleaved.chunks_exact(ch) {
            let (a, b) = if ch >= 2 { (frame[0], frame[1]) } else { (frame[0], frame[0]) };
            self.chan_a[self.fill] = a;
            self.chan_b[self.fill] = b;
            self.fill += 1;
            if self.fill == self.n {
                self.analyze_frame();
                self.fill = 0;
            }
            self.short_mid[self.short_fill] = (a + b) * 0.5;
            self.short_side[self.short_fill] = (a - b) * 0.5;
            self.short_fill += 1;
            if self.short_fill == self.short_n {
                self.analyze_short_frame();
                self.short_fill = 0;
            }
            if self.fp_pos < self.fp_len {
                self.fp_mid[self.fp_pos] = (a + b) * 0.5;
                self.fp_side[self.fp_pos] = (a - b) * 0.5;
                if self.fp_pos + 1 == self.fp_len {
                    self.fingerprint_frame();
                }
            }
            self.fp_pos += 1;
            if self.fp_pos == self.fp_hop.max(self.fp_len) {
                self.fp_pos = 0;
            }
        }
    }

    /// Windowed complex spectrum of `src` into (re, im).
    fn spectrum(fft: &mut RealFft, window: &[f32], src: &[f32], scratch: &mut [f32], re: &mut [f32], im: &mut [f32]) {
        for ((o, &x), &w) in scratch.iter_mut().zip(src.iter()).zip(window.iter()) {
            *o = x * w;
        }
        fft.forward(scratch, re, im);
    }

    fn analyze_frame(&mut self) {
        let n = self.n;
        let fs = self.sample_rate as f64;
        let stereo = self.channels >= 2;
        let ms_a: f64 = self.chan_a.iter().map(|&x| (x as f64) * (x as f64)).sum::<f64>();
        let ms_b: f64 = if stereo { self.chan_b.iter().map(|&x| (x as f64) * (x as f64)).sum::<f64>() } else { ms_a };
        let ms = (ms_a + ms_b) / (2.0 * n as f64);
        let frame_db = pdb(ms);
        let active = frame_db > ACTIVE_DB;
        Self::spectrum(&mut self.fft, &self.window, &self.chan_a, &mut self.scratch_in, &mut self.re_a, &mut self.im_a);
        let norm = self.norm;
        let has_side = stereo && self.chan_a.iter().zip(self.chan_b.iter()).any(|(a, b)| a != b);
        if stereo {
            Self::spectrum(&mut self.fft, &self.window, &self.chan_b, &mut self.scratch_in, &mut self.re_b, &mut self.im_b);
            for k in 0..self.p_mid.len() {
                let (ar, ai, br, bi) = (self.re_a[k], self.im_a[k], self.re_b[k], self.im_b[k]);
                self.p_mid[k] = 0.5 * (ar * ar + ai * ai + br * br + bi * bi) * norm;
                let (sr, si) = (ar - br, ai - bi);
                self.p_side[k] = 0.25 * (sr * sr + si * si) * norm;
            }
        } else {
            for k in 0..self.p_mid.len() {
                let (ar, ai) = (self.re_a[k], self.im_a[k]);
                self.p_mid[k] = (ar * ar + ai * ai) * norm;
            }
        }
        if has_side {
            self.side_seen = true;
        }
        let start_time = (self.frame_index * n as u64) as f64 / fs;

        if active {
            self.active_frames += 1;
            for (a, &p) in self.ltas_mid.iter_mut().zip(self.p_mid.iter()) {
                *a += p as f64;
            }
            if has_side {
                for (a, &p) in self.ltas_side.iter_mut().zip(self.p_side.iter()) {
                    *a += p as f64;
                }
            }
            self.store_hf(start_time);
        }
        if frame_db > -100.0 && frame_db < -40.0 {
            self.quiet_frames += 1;
            self.quiet_ms_sum += ms;
            for (a, &p) in self.quiet_ltas.iter_mut().zip(self.p_mid.iter()) {
                *a += p as f64;
            }
        }
        self.accumulate_spectrogram();
        self.frame_index += 1;
    }

    fn band_sum(p: &[f32], lo: usize, hi: usize) -> f64 {
        p[lo..hi.max(lo + 1).min(p.len())].iter().map(|&v| v as f64).sum()
    }

    fn store_hf(&mut self, time: f64) {
        self.hf_seen += 1;
        if !(self.hf_seen - 1).is_multiple_of(self.hf_stride) {
            return;
        }
        for w in self.hf_edges.windows(2) {
            let e = Self::band_sum(&self.p_mid, w[0], w[1]);
            self.hf_data.push(db_to_q(pdb(e)));
        }
        let (rl, rh) = self.ref_bins;
        self.hf_ref.push(db_to_q(pdb(Self::band_sum(&self.p_mid, rl, rh))));
        self.hf_times.push(time as f32);
        if self.hf_times.len() > MAX_STORED_FRAMES {
            // Keep every other stored frame and halve the storage rate.
            let nb = self.hf_edges.len() - 1;
            let mut data = Vec::with_capacity(self.hf_data.len() / 2 + nb);
            let mut refs = Vec::new();
            let mut times = Vec::new();
            for f in (0..self.hf_times.len()).step_by(2) {
                data.extend_from_slice(&self.hf_data[f * nb..(f + 1) * nb]);
                refs.push(self.hf_ref[f]);
                times.push(self.hf_times[f]);
            }
            self.hf_data = data;
            self.hf_ref = refs;
            self.hf_times = times;
            self.hf_stride *= 2;
        }
    }

    fn analyze_short_frame(&mut self) {
        let n = self.short_n;
        let e_mid: f64 = self.short_mid.iter().map(|&x| (x as f64) * (x as f64)).sum::<f64>() / n as f64;
        let e_side: f64 = self.short_side.iter().map(|&x| (x as f64) * (x as f64)).sum::<f64>() / n as f64;
        // Analyse the mid signal unless the side carries more energy
        // (e.g. a polarity-inverted channel).
        let (src, ms) = if e_side > e_mid { (&self.short_side, e_side) } else { (&self.short_mid, e_mid) };
        if pdb(ms) <= ACTIVE_DB {
            return;
        }
        for ((o, &x), &w) in self.short_in.iter_mut().zip(src.iter()).zip(self.short_window.iter()) {
            *o = x * w;
        }
        self.short_fft.power(&self.short_in, &mut self.p_short, &mut self.scratch_re, &mut self.scratch_im);
        let norm = self.short_norm;
        self.p_short.iter_mut().for_each(|p| *p *= norm);
        for sb in self.subbands.iter_mut() {
            let bins = &self.p_short[sb.lo_bin..sb.hi_bin];
            let count = bins.len() as f64;
            let mean: f64 = bins.iter().map(|&p| p as f64).sum::<f64>() / count;
            // Only frames where the sub-band carries content (> -110 dB per bin).
            if mean <= 1e-11 {
                continue;
            }
            self.sort_scratch.clear();
            self.sort_scratch.extend_from_slice(bins);
            let mid = self.sort_scratch.len() / 2;
            let (_, median, _) = self.sort_scratch.select_nth_unstable_by(mid, |a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let hole_thr = *median * HOLE_RATIO;
            let floor = (mean * 1e-9) as f32;
            let mut log_sum = 0f32;
            let mut holes = 0u32;
            for &p in bins {
                log_sum += fast_ln(p.max(floor));
                if p < hole_thr {
                    holes += 1;
                }
            }
            let log_sum = log_sum as f64;
            let sfm = ((log_sum / count).exp() / mean).clamp(0.0, 1.0);
            sb.frames += 1;
            sb.sfm_sum += sfm;
            sb.hole_sum += holes as f64 / count;
            let hb = ((sfm * SFM_HIST_BINS as f64) as usize).min(SFM_HIST_BINS - 1);
            sb.hist[hb] += 1;
        }
    }

    fn accumulate_spectrogram(&mut self) {
        for r in 0..SPECTROGRAM_ROWS {
            let (lo, hi) = (self.sg_row_edges[r], self.sg_row_edges[r + 1].max(self.sg_row_edges[r] + 1));
            let s = Self::band_sum(&self.p_mid, lo, hi) / (hi - lo) as f64;
            self.sg_acc[r] += s;
        }
        self.sg_frames += 1;
        if self.sg_frames == self.sg_per_col {
            self.push_column();
        }
    }

    fn push_column(&mut self) {
        let k = self.sg_frames.max(1) as f64;
        self.sg_cols.push(self.sg_acc.iter().map(|&v| (v / k) as f32).collect());
        self.sg_acc.iter_mut().for_each(|v| *v = 0.0);
        self.sg_frames = 0;
        if self.sg_cols.len() >= 2 * SPECTROGRAM_COLS {
            self.merge_columns();
        }
    }

    fn merge_columns(&mut self) {
        let merged: Vec<Vec<f32>> = self
            .sg_cols
            .chunks(2)
            .map(|pair| {
                if pair.len() == 2 {
                    pair[0].iter().zip(pair[1].iter()).map(|(a, b)| (a + b) * 0.5).collect()
                } else {
                    pair[0].clone()
                }
            })
            .collect();
        self.sg_cols = merged;
        self.sg_per_col *= 2;
    }

    /// One fingerprint frame: band energies of the stronger of mid/side
    /// (polarity-proof), compared with the previous frame.
    fn fingerprint_frame(&mut self) {
        let n = self.fp_len as f64;
        let e_mid: f64 = self.fp_mid.iter().map(|&x| (x as f64) * (x as f64)).sum::<f64>() / n;
        let e_side: f64 = self.fp_side.iter().map(|&x| (x as f64) * (x as f64)).sum::<f64>() / n;
        let (src, ms) = if e_side > e_mid { (&self.fp_side, e_side) } else { (&self.fp_mid, e_mid) };
        let audible = pdb(ms) > -70.0;
        let mut e = [0f32; FP_BANDS];
        if audible {
            for (i, o) in self.fp_in.iter_mut().enumerate() {
                *o = if i < self.fp_len { src[i] * self.fp_window[i] } else { 0.0 };
            }
            self.fp_fft.power(&self.fp_in, &mut self.fp_power, &mut self.scratch_re, &mut self.scratch_im);
            for (b, w) in self.fp_edges.windows(2).enumerate() {
                let (lo, hi) = (w[0], w[1].max(w[0] + 1).min(self.fp_power.len()));
                e[b] = (self.fp_power[lo..hi].iter().map(|&v| v as f64).sum::<f64>() + 1e-12).ln() as f32;
            }
        }
        let bits = match (&self.fp_prev, audible) {
            (Some(prev), true) => {
                let mut v = 0u32;
                for m in 0..FP_BANDS - 1 {
                    let d = (e[m] - e[m + 1]) - (prev[m] - prev[m + 1]);
                    if d > 0.0 {
                        v |= 1 << m;
                    }
                }
                // 0 is reserved for "no data".
                if v == 0 {
                    1
                } else {
                    v
                }
            }
            _ => 0,
        };
        self.fingerprint.push(bits);
        self.fp_prev = if audible { Some(e) } else { None };
    }

    pub fn finish(mut self) -> SpectralResult {
        let fs = self.sample_rate as f64;
        let n = self.n;
        let bin_hz = fs / n as f64;
        let short_bin_hz = fs / self.short_n as f64;
        let nyq = fs / 2.0;
        if self.sg_frames > 0 {
            self.push_column();
        }
        while self.sg_cols.len() > SPECTROGRAM_COLS {
            self.merge_columns();
        }
        let af = self.active_frames.max(1) as f64;
        let to_db = |v: &Vec<f64>, k: f64| -> Vec<f32> { v.iter().map(|&p| pdb(p / k) as f32).collect() };
        let ltas_db = to_db(&self.ltas_mid, af);
        let ltas_side_db = if self.side_seen { Some(to_db(&self.ltas_side, af)) } else { None };
        let quiet_ltas_db = if self.quiet_frames > 0 { Some(to_db(&self.quiet_ltas, self.quiet_frames as f64)) } else { None };

        let subbands = self
            .subbands
            .iter()
            .map(|sb| {
                let mut median = 0.0;
                if sb.frames > 0 {
                    let half = sb.frames.div_ceil(2);
                    let mut acc = 0u64;
                    for (i, &h) in sb.hist.iter().enumerate() {
                        acc += h as u64;
                        if acc >= half {
                            median = (i as f64 + 0.5) / SFM_HIST_BINS as f64;
                            break;
                        }
                    }
                }
                SubbandStats {
                    lo_hz: sb.lo_bin as f64 * short_bin_hz,
                    hi_hz: sb.hi_bin as f64 * short_bin_hz,
                    frames: sb.frames,
                    mean_sfm: if sb.frames > 0 { sb.sfm_sum / sb.frames as f64 } else { 0.0 },
                    median_sfm: median,
                    mean_hole_ratio: if sb.frames > 0 { sb.hole_sum / sb.frames as f64 } else { 0.0 },
                }
            })
            .collect();

        // Spectrogram bytes.
        let cols = self.sg_cols.len();
        let mut data = Vec::with_capacity(cols * SPECTROGRAM_ROWS);
        for c in &self.sg_cols {
            for &p in c {
                let db = pdb(p as f64);
                let q = ((db - SPECTROGRAM_DB_MIN) / (SPECTROGRAM_DB_MAX - SPECTROGRAM_DB_MIN) * 255.0).round().clamp(0.0, 255.0);
                data.push(q as u8);
            }
        }
        let secs_per_col = (self.sg_per_col * n) as f64 / fs;

        // Band energy distribution from the mid LTAS.
        const BANDS: [(&str, &str, f64, f64); 9] = [
            ("sub", "Sub-bass", 20.0, 60.0),
            ("bass", "Bass", 60.0, 250.0),
            ("low_mid", "Low mids", 250.0, 500.0),
            ("mid", "Mids", 500.0, 2000.0),
            ("high_mid", "High mids", 2000.0, 4000.0),
            ("presence", "Presence", 4000.0, 6000.0),
            ("brilliance", "Brilliance", 6000.0, 12000.0),
            ("air", "Air", 12000.0, 20000.0),
            ("ultrasonic", "Above 20 kHz", 20000.0, f64::INFINITY),
        ];
        let bin_of = |hz: f64| ((hz / bin_hz).round() as usize).min(self.ltas_mid.len());
        let total: f64 = self.ltas_mid[bin_of(20.0)..].iter().sum::<f64>().max(1e-30);
        let bands = BANDS
            .iter()
            .filter(|(id, _, lo, _)| *lo < nyq && !(*id == "ultrasonic" && nyq < 20_500.0))
            .map(|&(id, label, lo, hi)| {
                let e: f64 = self.ltas_mid[bin_of(lo)..bin_of(hi.min(nyq))].iter().sum();
                BandEnergy { id, label, lo_hz: lo, hi_hz: hi.min(nyq), share: e / total, rel_db: pdb(e / total) }
            })
            .collect();

        // Log-spaced display spectrum (20 Hz … Nyquist).
        let mut display = Vec::with_capacity(LOG_SPECTRUM_POINTS);
        let (f_lo, f_hi) = (20f64, nyq);
        let step = (f_hi / f_lo).powf(1.0 / LOG_SPECTRUM_POINTS as f64);
        for i in 0..LOG_SPECTRUM_POINTS {
            let a = f_lo * step.powi(i as i32);
            let b = a * step;
            let (lb, hb) = ((a / bin_hz).floor() as usize, ((b / bin_hz).ceil() as usize).min(self.ltas_mid.len() - 1));
            let hb = hb.max(lb + 1).min(self.ltas_mid.len());
            let lb = lb.min(hb - 1);
            let mean = |v: &Vec<f64>| v[lb..hb].iter().sum::<f64>() / (hb - lb) as f64 / af;
            let mid_db = pdb(mean(&self.ltas_mid)) as f32;
            let side_db = if self.side_seen { Some(pdb(mean(&self.ltas_side)) as f32) } else { None };
            display.push(((a * b).sqrt() as f32, mid_db, side_db));
        }

        SpectralResult {
            fft_size: n,
            bin_hz,
            nyquist: nyq,
            frame_secs: n as f64 / fs,
            frames: self.frame_index,
            active_frames: self.active_frames,
            ltas_db,
            ltas_side_db,
            quiet_ltas_db,
            quiet_frames: self.quiet_frames,
            quiet_rms_db: if self.quiet_frames > 0 { Some(pdb(self.quiet_ms_sum / self.quiet_frames as f64)) } else { None },
            hf: HfFrames {
                start_hz: HF_START_HZ,
                band_hz: self.hf_band_hz,
                bands: self.hf_edges.len() - 1,
                data: self.hf_data,
                reference: self.hf_ref,
                times: self.hf_times,
            },
            subbands,
            spectrogram: Spectrogram { cols, rows: SPECTROGRAM_ROWS, data, secs_per_col, max_hz: nyq },
            fingerprint: self.fingerprint,
            bands,
            display,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic white noise in [-1, 1).
    pub fn noise(seed: u64, n: usize) -> Vec<f32> {
        let mut s = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (0..n)
            .map(|_| {
                s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                ((s >> 40) as f64 / (1u64 << 24) as f64 * 2.0 - 1.0) as f32
            })
            .collect()
    }

    #[test]
    fn white_noise_is_flat_with_natural_flatness() {
        let fs = 44_100;
        let x: Vec<f32> = noise(7, fs as usize * 10).iter().map(|v| v * 0.3).collect();
        let mut a = SpectralAnalyzer::new(fs, 1, Some(x.len() as u64));
        a.process(&x);
        let r = a.finish();
        // Flat spectrum: 1 kHz and 18 kHz within 1 dB.
        let b1 = (1000.0 / r.bin_hz) as usize;
        let b2 = (18000.0 / r.bin_hz) as usize;
        let avg = |b: usize| r.ltas_db[b - 5..b + 5].iter().map(|&v| v as f64).sum::<f64>() / 10.0;
        assert!((avg(b1) - avg(b2)).abs() < 1.0);
        // Spectral flatness of complex-Gaussian bins ≈ e^-γ ≈ 0.56; few holes.
        for sb in &r.subbands {
            assert!(sb.mean_sfm > 0.45 && sb.mean_sfm < 0.62, "{} Hz: sfm {}", sb.lo_hz, sb.mean_sfm);
            assert!(sb.mean_hole_ratio < 0.03, "holes {}", sb.mean_hole_ratio);
        }
        assert!(r.spectrogram.cols > 0 && r.spectrogram.cols <= SPECTROGRAM_COLS);
        assert_eq!(r.spectrogram.data.len(), r.spectrogram.cols * SPECTROGRAM_ROWS);
        // ~10 fingerprint frames per second.
        assert!((r.fingerprint.len() as i64 - 100).abs() <= 2, "{}", r.fingerprint.len());
        assert!(r.hf.frames() > 0);
    }

    #[test]
    fn sine_lands_at_zero_db() {
        let fs = 48_000;
        let f = 1500.0;
        let x: Vec<f32> = (0..fs as usize * 2).map(|i| (2.0 * std::f64::consts::PI * f * i as f64 / fs as f64).sin() as f32).collect();
        let mut a = SpectralAnalyzer::new(fs, 1, None);
        a.process(&x);
        let r = a.finish();
        let peak = r.ltas_db.iter().cloned().fold(f32::MIN, f32::max);
        assert!(peak > -1.5 && peak < 0.5, "peak {peak}");
    }

    #[test]
    fn fast_ln_is_accurate() {
        for &x in &[1e-12f32, 1e-6, 0.001, 0.37, 1.0, 1.5, 2.0, 1000.0, 3.3e7] {
            assert!((fast_ln(x) - x.ln()).abs() < 2e-4, "{x}: {} vs {}", fast_ln(x), x.ln());
        }
    }

    #[test]
    fn identical_stereo_has_no_side() {
        let fs = 44_100;
        let m = noise(3, fs as usize * 2);
        let mut x = Vec::new();
        for v in &m {
            x.push(*v * 0.2);
            x.push(*v * 0.2);
        }
        let mut a = SpectralAnalyzer::new(fs, 2, None);
        a.process(&x);
        assert!(a.finish().ltas_side_db.is_none());
    }
}
