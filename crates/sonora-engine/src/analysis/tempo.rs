//! Tempo (BPM) estimation.
//!
//! One streaming pass computes an onset-strength envelope: the spectral flux
//! of log-compressed band magnitudes (four bands per octave from 30 Hz) in
//! ~23 ms Hann frames every 10 ms. At the end of the track:
//!
//! 1. the envelope's local mean is removed and the rest half-wave rectified,
//!    leaving the onsets;
//! 2. its autocorrelation, summed over the first four multiples of each beat
//!    period (a comb), gives a tempo salience over 40–250 BPM whose peaks are
//!    the candidate tempos (the beat, half and double time, bar periods…);
//! 3. the candidate preferred inside the default 70–180 BPM range (salience
//!    times a mild preference for tempos near 120 BPM) is refined by folding
//!    the whole envelope at finely spaced beat periods and keeping the period
//!    that lines the onsets up most sharply. Over a whole track that pins a
//!    steady tempo down to a few thousandths of a BPM;
//! 4. local tempos in 20 s windows show whether the tempo holds.
//!
//! Tracks without a regular pulse (ambient, speech, noise, a lone tone) get no
//! tempo rather than a guess. Which octave of the pulse is "the" tempo (87 or
//! 174 for drum & bass) is a convention, not a measurement: the candidates are
//! kept so a different BPM range can be applied later without decoding again
//! ([`resolve_bpm`]).

use crate::dsp::fft::RealFft;
use crate::dsp::hann;

/// Onset-envelope frames per second (approximately: the hop is a whole number of samples).
const FRAME_RATE: f64 = 100.0;
/// Range searched for candidate tempos.
pub const MIN_BPM: f64 = 40.0;
pub const MAX_BPM: f64 = 250.0;
/// Range the reported tempo is chosen from, unless another is applied later.
pub const DEFAULT_RANGE: (f64, f64) = (70.0, 180.0);
/// Mild preference for tempos near this when a range holds two octaves of one pulse.
const PREFERRED_BPM: f64 = 120.0;
/// Width of that preference, in octaves (one standard deviation).
const PREFERENCE_OCTAVES: f64 = 1.0;
/// Only the first 30 minutes are used (enough for any track; DJ mixes have no single tempo).
const MAX_SECONDS: f64 = 1800.0;
const BANDS_PER_OCTAVE: f64 = 4.0;
const BAND_LO_HZ: f64 = 30.0;
const BAND_HI_HZ: f64 = 16_000.0;
/// Log compression of band magnitudes (relative to full scale) before differencing.
const COMPRESSION: f32 = 1000.0;
/// Shortest stretch of audio given a tempo.
const MIN_SECONDS: f64 = 8.0;
/// Below this fold contrast there is no regular pulse.
const MIN_CONTRAST: f64 = 0.9;
/// Onsets weaker than this (log-magnitude flux, at the 99th percentile) are
/// not rhythm at all: a slow swell or fade, a sustained chord.
const MIN_ONSET: f32 = 0.3;
/// Readings less confident than this are not reported.
const MIN_CONFIDENCE: f64 = 0.15;

#[derive(Clone, Debug, PartialEq)]
pub struct TempoResult {
    /// Average tempo of the track, read within [`DEFAULT_RANGE`].
    pub bpm: f64,
    /// 0–1: how clearly and how steadily the beat stands out.
    pub confidence: f64,
    /// The tempo changes noticeably along the track (live playing, tempo changes).
    pub variable: bool,
    /// Readings of the pulse at other metrical levels (half, double, 2/3…) and
    /// other salience peaks, as (bpm, salience 0–1), strongest first.
    pub candidates: Vec<(f64, f64)>,
}

/// Streaming onset-envelope collector; [`TempoMeter::finish`] estimates the tempo.
pub struct TempoMeter {
    ch: usize,
    n: usize,
    hop: usize,
    fr: f64,
    fft: RealFft,
    window: Vec<f32>,
    mono: Vec<f32>,
    /// Bin ranges `[lo, hi)` of the log-spaced bands.
    bands: Vec<(usize, usize)>,
    prev: Vec<f32>,
    cur: Vec<f32>,
    have_prev: bool,
    env: Vec<f32>,
    max_frames: usize,
    buf: Vec<f32>,
    re: Vec<f32>,
    im: Vec<f32>,
    /// Power normalisation: a full-scale sine reads 1 in its peak bin.
    scale: f32,
}

/// Power of two nearest to `x` (in the log domain).
fn nearest_pow2(x: f64) -> usize {
    1usize << (x.max(1.0).log2().round() as u32)
}

impl TempoMeter {
    pub fn new(fs: u32, ch: usize) -> TempoMeter {
        let fs_f = fs.max(1) as f64;
        let n = nearest_pow2(fs_f * 0.023).clamp(256, 4096);
        let hop = (fs_f / FRAME_RATE).round().max(1.0) as usize;
        let df = fs_f / n as f64;
        let top = BAND_HI_HZ.min(fs_f * 0.5 * 0.95);
        let mut bands: Vec<(usize, usize)> = Vec::new();
        let mut k = 0.0;
        loop {
            let lo = BAND_LO_HZ * 2f64.powf(k / BANDS_PER_OCTAVE);
            if lo >= top {
                break;
            }
            let hi = (lo * 2f64.powf(1.0 / BANDS_PER_OCTAVE)).min(top);
            let mut b0 = ((lo / df).round() as usize).max(1);
            let b1 = ((hi / df).round() as usize).min(n / 2);
            if let Some(&(_, prev_hi)) = bands.last() {
                b0 = b0.max(prev_hi);
            }
            if b1 > b0 {
                bands.push((b0, b1));
            }
            k += 1.0;
        }
        let w_sum = n as f32 / 2.0;
        TempoMeter {
            ch: ch.max(1),
            n,
            hop,
            fr: fs_f / hop as f64,
            fft: RealFft::new(n),
            window: hann(n),
            mono: Vec::with_capacity(n + 32_768),
            prev: vec![0.0; bands.len()],
            cur: vec![0.0; bands.len()],
            bands,
            have_prev: false,
            env: Vec::new(),
            max_frames: (MAX_SECONDS * fs_f / hop as f64) as usize,
            buf: vec![0.0; n],
            re: vec![0.0; n / 2 + 1],
            im: vec![0.0; n / 2 + 1],
            scale: 1.0 / (w_sum / 2.0).powi(2),
        }
    }

    /// Frames of the onset envelope per second.
    pub fn frame_rate(&self) -> f64 {
        self.fr
    }

    /// Feeds interleaved samples (`ch` channels).
    pub fn process(&mut self, data: &[f32]) {
        if self.env.len() >= self.max_frames {
            return;
        }
        let inv = 1.0 / self.ch as f32;
        for fr in data.chunks_exact(self.ch) {
            self.mono.push(fr.iter().sum::<f32>() * inv);
        }
        let mut pos = 0;
        while pos + self.n <= self.mono.len() {
            self.frame(pos);
            pos += self.hop;
            if self.env.len() >= self.max_frames {
                self.mono.clear();
                return;
            }
        }
        self.mono.drain(..pos);
    }

    fn frame(&mut self, pos: usize) {
        let x = &self.mono[pos..pos + self.n];
        for ((b, &v), &w) in self.buf.iter_mut().zip(x).zip(&self.window) {
            *b = v * w;
        }
        self.fft.forward(&self.buf, &mut self.re, &mut self.im);
        for (c, &(lo, hi)) in self.cur.iter_mut().zip(&self.bands) {
            let mut s = 0.0f32;
            for k in lo..hi {
                s += self.re[k] * self.re[k] + self.im[k] * self.im[k];
            }
            let mag = (s / (hi - lo) as f32 * self.scale).sqrt();
            *c = (1.0 + COMPRESSION * mag).ln();
        }
        let flux = if self.have_prev { self.cur.iter().zip(&self.prev).map(|(c, p)| (c - p).max(0.0)).sum() } else { 0.0 };
        self.env.push(flux);
        std::mem::swap(&mut self.prev, &mut self.cur);
        self.have_prev = true;
    }

    /// Estimates the tempo from everything fed so far.
    pub fn finish(self) -> Option<TempoResult> {
        estimate(&self.env, self.fr)
    }
}

/// Removes the local mean (over ±`half` frames) and keeps the positive part.
fn onsets(raw: &[f32], half: usize) -> Vec<f32> {
    let n = raw.len();
    let mut prefix = Vec::with_capacity(n + 1);
    prefix.push(0.0f64);
    for &v in raw {
        prefix.push(prefix.last().copied().unwrap_or(0.0) + v as f64);
    }
    (0..n)
        .map(|t| {
            let a = t.saturating_sub(half);
            let b = (t + half + 1).min(n);
            let mean = (prefix[b] - prefix[a]) / (b - a) as f64;
            (raw[t] as f64 - mean).max(0.0) as f32
        })
        .collect()
}

/// Unbiased autocorrelation for lags `0..=max_lag`, normalised to lag 0.
fn autocorrelation(o: &[f32], max_lag: usize) -> Vec<f64> {
    let n = o.len();
    let mut r = vec![0.0f64; max_lag + 1];
    for (lag, slot) in r.iter_mut().enumerate() {
        if lag >= n {
            break;
        }
        let a = &o[..n - lag];
        let b = &o[lag..];
        let mut acc = 0.0f64;
        // Partial sums in f32 (vectorised), flushed to f64 to keep precision.
        for (ca, cb) in a.chunks(4096).zip(b.chunks(4096)) {
            let s: f32 = ca.iter().zip(cb).map(|(x, y)| x * y).sum();
            acc += s as f64;
        }
        *slot = acc / (n - lag) as f64;
    }
    let r0 = r[0].max(1e-30);
    r.iter().map(|v| v / r0).collect()
}

fn interp(v: &[f64], x: f64) -> f64 {
    if x < 0.0 {
        return 0.0;
    }
    let i = x.floor() as usize;
    if i + 1 >= v.len() {
        return v.last().copied().unwrap_or(0.0);
    }
    let f = x - i as f64;
    v[i] * (1.0 - f) + v[i + 1] * f
}

fn preference(bpm: f64) -> f64 {
    let o = (bpm / PREFERRED_BPM).log2() / PREFERENCE_OCTAVES;
    (-0.5 * o * o).exp()
}

/// Phase histogram of the onsets folded at `bpm`: mean onset strength per
/// phase bin (one bin per ~2 frames, at least 12).
fn fold(o: &[f32], fr: f64, bpm: f64, out: &mut Vec<f64>) {
    let period = 60.0 * fr / bpm;
    let bins = ((period / 2.0).round() as usize).clamp(12, 64);
    out.clear();
    out.resize(bins, 0.0);
    let mut counts = vec![0u32; bins];
    let inv = 1.0 / period;
    for (t, &v) in o.iter().enumerate() {
        let ph = (t as f64 * inv).fract();
        let b = ((ph * bins as f64) as usize).min(bins - 1);
        out[b] += v as f64;
        counts[b] += 1;
    }
    for (s, &c) in out.iter_mut().zip(&counts) {
        *s /= c.max(1) as f64;
    }
}

/// How sharply the onsets line up at `bpm`: (peak − median) / mean of the phase histogram.
fn contrast(o: &[f32], fr: f64, bpm: f64, scratch: &mut Vec<f64>) -> f64 {
    fold(o, fr, bpm, scratch);
    let mean = scratch.iter().sum::<f64>() / scratch.len() as f64;
    if mean <= 0.0 {
        return 0.0;
    }
    let peak = scratch.iter().copied().fold(0.0, f64::max);
    let mut sorted = scratch.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = sorted[sorted.len() / 2];
    (peak - median) / mean
}

/// Tempo within `[lo, hi]` with the sharpest fold, searched in steps of `step` BPM.
fn sharpest(o: &[f32], fr: f64, lo: f64, hi: f64, step: f64, scratch: &mut Vec<f64>) -> (f64, f64) {
    let mut best = (lo, f64::NEG_INFINITY);
    let mut b = lo;
    while b <= hi + 1e-9 {
        let c = contrast(o, fr, b, scratch);
        if c > best.1 {
            best = (b, c);
        }
        b += step;
    }
    best
}

/// Simple ratios between readings of one pulse.
const RATIOS: [f64; 11] = [0.25, 1.0 / 3.0, 0.5, 2.0 / 3.0, 0.75, 1.0, 4.0 / 3.0, 1.5, 2.0, 3.0, 4.0];

/// The ratio `c / reference` is close to (within 2 %).
fn related(c: f64, reference: f64) -> Option<f64> {
    let q = c / reference;
    RATIOS.iter().copied().find(|r| (q / r - 1.0).abs() < 0.02)
}

/// Estimates the tempo from an onset-strength envelope sampled at `fr` Hz.
pub fn estimate(raw: &[f32], fr: f64) -> Option<TempoResult> {
    if (raw.len() as f64) < MIN_SECONDS * fr {
        return None;
    }
    let o = onsets(raw, (fr * 0.5) as usize);
    let mean_o = o.iter().map(|&v| v as f64).sum::<f64>() / o.len() as f64;
    if mean_o <= 1e-9 {
        return None;
    }
    let mut sorted_o = o.clone();
    sorted_o.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    if sorted_o[(sorted_o.len() as f64 * 0.99) as usize] < MIN_ONSET {
        return None;
    }
    drop(sorted_o);

    // Comb salience over the BPM grid.
    let max_lag = (4.0 * 60.0 * fr / MIN_BPM).ceil() as usize + 2;
    let acf = autocorrelation(&o, max_lag.min(o.len().saturating_sub(1)));
    const STEP: f64 = 0.05;
    let grid: Vec<f64> = (0..).map(|i| MIN_BPM + i as f64 * STEP).take_while(|b| *b <= MAX_BPM + 1e-9).collect();
    let sal: Vec<f64> = grid
        .iter()
        .map(|&b| {
            let p = 60.0 * fr / b;
            (1..=4).map(|m| interp(&acf, m as f64 * p) / m as f64).sum::<f64>()
        })
        .collect();
    let mut sorted = sal.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let base = sorted[sorted.len() / 2];
    let top = sorted.last().copied().unwrap_or(0.0);
    if top - base <= 1e-12 {
        return None;
    }
    let norm: Vec<f64> = sal.iter().map(|s| ((s - base) / (top - base)).max(0.0)).collect();

    // Peaks: maxima within ±1.5 % of their own tempo.
    let mut peaks: Vec<(f64, f64)> = Vec::new();
    for (i, &b) in grid.iter().enumerate() {
        let w = ((0.015 * b / STEP).round() as usize).max(2);
        let lo = i.saturating_sub(w);
        let hi = (i + w + 1).min(grid.len());
        if norm[i] > 0.0 && norm[lo..hi].iter().all(|&v| v <= norm[i]) && peaks.last().is_none_or(|p| (b - p.0).abs() > STEP * 1.5) {
            peaks.push((b, norm[i]));
        }
    }
    peaks.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    peaks.truncate(8);
    let (lo, hi) = DEFAULT_RANGE;
    let mut pick = peaks
        .iter()
        .filter(|p| p.0 >= lo && p.0 <= hi)
        .max_by(|a, b| (a.1 * preference(a.0)).partial_cmp(&(b.1 * preference(b.0))).unwrap_or(std::cmp::Ordering::Equal))
        .copied()
        .or_else(|| peaks.first().map(|&(b, s)| (fold_into(b, lo, hi), s)))?;

    // A periodic pattern correlates as well at two beats as at one, so the
    // comb cannot tell a pulse from its half. Folding can: when the onsets
    // half way between the "beats" are about as strong as the beats
    // themselves, the pulse is twice as fast (a 70 BPM reading of a
    // half-time 140 track, or of a plain 174 BPM beat).
    let mut scratch = Vec::new();
    if pick.0 * 2.0 <= hi {
        fold(&o, fr, pick.0, &mut scratch);
        let bins = scratch.len();
        let mut sorted = scratch.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let median = sorted[bins / 2];
        let p = scratch.iter().enumerate().max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal)).map(|(i, _)| i).unwrap_or(0);
        let opposite = (0..3).map(|d| scratch[(p + bins / 2 + bins - 1 + d) % bins]).fold(0.0, f64::max);
        if scratch[p] > median && (opposite - median) >= 0.6 * (scratch[p] - median) {
            pick = (pick.0 * 2.0, interp_norm(&grid, &norm, pick.0 * 2.0));
        }
    }

    // Local tempos in 20 s windows (10 s apart) around the candidate: they
    // show whether the tempo holds, and still find a drifting pulse.
    let win = ((20.0 * fr) as usize).min(o.len());
    let mut local = Vec::new();
    let mut active = 0usize;
    let mut start = 0;
    while start + win <= o.len() {
        let w = &o[start..start + win];
        let m = w.iter().map(|&v| v as f64).sum::<f64>() / win as f64;
        if m >= 0.5 * mean_o {
            active += 1;
            let (b, c) = sharpest(w, fr, pick.0 * 0.88, pick.0 * 1.12, pick.0 * 0.0025, &mut scratch);
            if c >= MIN_CONTRAST {
                local.push(b);
            }
        }
        start += (win / 2).max(1);
    }

    // Refine by folding the whole envelope at finely spaced periods.
    let coarse = sharpest(&o, fr, pick.0 * 0.975, pick.0 * 1.025, 0.025, &mut scratch);
    let (mut bpm, k) = sharpest(&o, fr, coarse.0 - 0.03, coarse.0 + 0.03, 0.002, &mut scratch);
    let steady = k >= MIN_CONTRAST;
    let support;
    if steady {
        // Most dance music is produced at a whole-number tempo.
        let whole = bpm.round();
        if (bpm - whole).abs() <= 0.1 && contrast(&o, fr, whole, &mut scratch) >= 0.98 * k {
            bpm = whole;
        }
        // The fold must be supported by many beats, not one loud event.
        support = beat_support(&o, fr, bpm, &mut scratch);
        if support < 0.25 {
            return None;
        }
    } else {
        // No single tempo lines the whole track up. A pulse that drifts still
        // shows up locally; anything else has no tempo.
        if local.len() < 3 || local.len() * 2 < active {
            return None;
        }
        let mut sorted = local.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        bpm = sorted[sorted.len() / 2];
        support = local.len() as f64 / active.max(1) as f64;
    }
    let off = local.iter().filter(|&&b| (b / bpm - 1.0).abs() > 0.015).count();
    let off_fraction = if local.len() >= 3 { off as f64 / local.len() as f64 } else { 0.0 };
    let variable = !steady || (off >= 2 && off_fraction >= 0.2);

    let clarity = if steady { ((k - MIN_CONTRAST) / 3.0).clamp(0.0, 1.0) } else { 0.5 };
    let confidence = (clarity * (0.4 + 0.6 * support.min(1.0)) * (1.0 - 0.6 * off_fraction) * if steady { 1.0 } else { 0.6 }).clamp(0.0, 1.0);
    if confidence < MIN_CONFIDENCE {
        return None;
    }

    // Candidates, expressed exactly relative to the refined tempo when related.
    let mut candidates: Vec<(f64, f64)> = vec![(bpm, interp_norm(&grid, &norm, bpm))];
    for &(b, s) in &peaks {
        let exact = related(b, bpm).map(|r| bpm * r).unwrap_or(b);
        if candidates.iter().all(|c| (c.0 / exact - 1.0).abs() > 0.01) {
            candidates.push((exact, s));
        }
    }
    for r in [0.5, 2.0] {
        let b = bpm * r;
        if (MIN_BPM..=MAX_BPM).contains(&b) && candidates.iter().all(|c| (c.0 / b - 1.0).abs() > 0.01) {
            candidates.push((b, interp_norm(&grid, &norm, b)));
        }
    }
    candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    candidates.truncate(6);
    let round = |v: f64, d: i32| (v * 10f64.powi(d)).round() / 10f64.powi(d);
    Some(TempoResult {
        bpm: round(bpm, 3),
        confidence: round(confidence, 3),
        variable,
        candidates: candidates.into_iter().map(|(b, s)| (round(b, 3), round(s, 3))).collect(),
    })
}

fn interp_norm(grid: &[f64], norm: &[f64], bpm: f64) -> f64 {
    let x = (bpm - grid[0]) / (grid[1] - grid[0]);
    interp(norm, x).clamp(0.0, 1.0)
}

/// Fraction of beats (at the strongest phase of `bpm`) carrying a clear onset.
fn beat_support(o: &[f32], fr: f64, bpm: f64, scratch: &mut Vec<f64>) -> f64 {
    fold(o, fr, bpm, scratch);
    let bins = scratch.len();
    let peak_bin = scratch.iter().enumerate().max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal)).map(|(i, _)| i).unwrap_or(0);
    let period = 60.0 * fr / bpm;
    let phase = (peak_bin as f64 + 0.5) / bins as f64 * period;
    let mut sorted: Vec<f32> = o.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = sorted[sorted.len() / 2] as f64;
    let threshold = median + 0.5 * (o.iter().map(|&v| v as f64).sum::<f64>() / o.len() as f64);
    let mut beats = 0usize;
    let mut hits = 0usize;
    let mut t = phase;
    let reach = (period / bins as f64).max(1.0).ceil() as usize + 1;
    while (t as usize) < o.len() {
        let c = t.round() as usize;
        let a = c.saturating_sub(reach);
        let b = (c + reach + 1).min(o.len());
        if a < b {
            beats += 1;
            if o[a..b].iter().any(|&v| v as f64 > threshold) {
                hits += 1;
            }
        }
        t += period;
    }
    if beats == 0 {
        0.0
    } else {
        hits as f64 / beats as f64
    }
}

/// Moves `bpm` by octaves into `[lo, hi]` (as close as possible when the range is narrower than an octave).
pub fn fold_into(mut bpm: f64, lo: f64, hi: f64) -> f64 {
    if bpm <= 0.0 || !bpm.is_finite() {
        return bpm;
    }
    while bpm < lo && bpm * 2.0 <= hi * 1.0001 {
        bpm *= 2.0;
    }
    while bpm > hi && bpm / 2.0 >= lo * 0.9999 {
        bpm /= 2.0;
    }
    bpm
}

/// The tempo to show for a BPM range other than [`DEFAULT_RANGE`]: the
/// detected tempo when it lies in the range, otherwise the same pulse moved
/// by octaves into it, otherwise the strongest candidate inside it.
pub fn resolve_bpm(bpm: f64, candidates: &[(f64, f64)], lo: f64, hi: f64) -> f64 {
    if bpm >= lo && bpm <= hi {
        return bpm;
    }
    let folded = fold_into(bpm, lo, hi);
    if folded >= lo && folded <= hi {
        return folded;
    }
    candidates
        .iter()
        .filter(|c| c.0 >= lo && c.0 <= hi)
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|c| c.0)
        .unwrap_or(folded)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Onset envelope of a click track: one onset per beat (plus optional off-beats).
    fn clicks(bpm: f64, seconds: f64, offbeat: f32, fr: f64) -> Vec<f32> {
        let n = (seconds * fr) as usize;
        let mut env = vec![0.01f32; n];
        let period = 60.0 * fr / bpm;
        let mut t = 3.0f64;
        while (t as usize) < n {
            let i = t.round() as usize;
            if i < n {
                env[i] += 1.0;
            }
            let j = (t + period / 2.0).round() as usize;
            if j < n {
                env[j] += offbeat;
            }
            t += period;
        }
        env
    }

    #[test]
    fn click_tracks_read_exactly() {
        for bpm in [92.0, 124.0, 128.0, 140.0, 172.0, 123.45] {
            let r = estimate(&clicks(bpm, 120.0, 0.3, 100.0), 100.0).unwrap_or_else(|| panic!("{bpm}: no tempo"));
            assert!((r.bpm - bpm).abs() < 0.02, "{bpm}: got {}", r.bpm);
            assert!(r.confidence > 0.6, "{bpm}: confidence {}", r.confidence);
            assert!(!r.variable);
        }
    }

    #[test]
    fn octave_follows_the_range() {
        let r = estimate(&clicks(174.0, 90.0, 0.2, 100.0), 100.0).unwrap();
        assert!((r.bpm - 174.0).abs() < 0.02, "{}", r.bpm);
        assert!((resolve_bpm(r.bpm, &r.candidates, 60.0, 120.0) - 87.0).abs() < 0.02);
        assert!((resolve_bpm(r.bpm, &r.candidates, 88.0, 175.0) - 174.0).abs() < 0.02);
        assert!((resolve_bpm(128.0, &[], 60.0, 120.0) - 64.0).abs() < 1e-9);
        assert!((resolve_bpm(64.0, &[], 100.0, 200.0) - 128.0).abs() < 1e-9);
        assert_eq!(fold_into(300.0, 70.0, 180.0), 150.0);
    }

    #[test]
    fn noise_and_silence_have_no_tempo() {
        let mut x = 12345u64;
        let noise: Vec<f32> = (0..6000)
            .map(|_| {
                x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
                (x >> 40) as f32 / (1u64 << 24) as f32
            })
            .collect();
        assert!(estimate(&noise, 100.0).is_none());
        assert!(estimate(&vec![0.0; 6000], 100.0).is_none());
        // A single event is not a pulse.
        let mut one = vec![0.0f32; 6000];
        one[1000] = 5.0;
        assert!(estimate(&one, 100.0).is_none());
        assert!(estimate(&clicks(120.0, 5.0, 0.0, 100.0), 100.0).is_none(), "too short");
    }

    #[test]
    fn drifting_tempo_is_flagged() {
        // 120 → 132 BPM over three minutes.
        let fr = 100.0;
        let n = (180.0 * fr) as usize;
        let mut env = vec![0.01f32; n];
        let mut t = 0.0f64;
        while (t as usize) < n {
            env[t as usize] += 1.0;
            let bpm = 120.0 + 12.0 * t / n as f64;
            t += 60.0 * fr / bpm;
        }
        let r = estimate(&env, fr).unwrap();
        assert!(r.variable, "{r:?}");
        assert!(r.confidence < 0.6, "{r:?}");
    }
}
