//! Bandwidth, cutoff and artifact measurements derived from
//! [`SpectralResult`](super::spectral::SpectralResult).
//!
//! These are *measurements* (what the spectrum looks like). Turning them into
//! conclusions about provenance happens in [`crate::assess`], which is
//! deliberately conservative: a steep cutoff is a clue, not proof.

use super::spectral::{q_to_db, HfFrames, SpectralResult, SubbandStats};
use crate::util::{median, percentile_sorted};

/// Minimum drop (dB) across an edge for it to be called a steep cutoff.
pub const STEEP_DROP_DB: f64 = 30.0;
/// Maximum transition width (Hz) for a steep cutoff. Encoder low-passes
/// measured on real encodes span roughly 0.7–1.8 kHz at this resolution.
pub const STEEP_TRANSITION_HZ: f64 = 2000.0;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Edge {
    /// Frequency where the spectrum falls halfway (in dB) from shelf to floor.
    pub hz: f64,
    /// Shelf level minus floor level (dB).
    pub drop_db: f64,
    /// Raw edge score: mean level below minus mean level above (dB).
    pub edge_db: f64,
    pub shelf_db: f64,
    pub floor_db: f64,
    /// Width of the transition from shelf-3 dB to floor+3 dB (Hz).
    pub transition_hz: f64,
    /// Standard deviation of the spectrum above the edge (dB): small for a
    /// flat, empty floor.
    pub floor_stdev_db: f64,
}

impl Edge {
    pub fn is_steep(&self) -> bool {
        self.drop_db >= STEEP_DROP_DB && self.transition_hz <= STEEP_TRANSITION_HZ
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BandwidthMeasure {
    /// Strongest spectral edge between 4 kHz and Nyquist (channel-average
    /// spectrum; see `spectral`).
    pub edge: Option<Edge>,
    /// Highest frequency with content at least 10 dB above the floor.
    pub upper_hz: f64,
    pub nyquist: f64,
    /// Fraction of analysed frames whose energy above the edge stays at
    /// least 30 dB below the energy just under it.
    pub ceiling_consistency: Option<f64>,
    /// 99th percentile of (energy above edge − energy below edge), dB.
    pub breakthrough_p99_db: Option<f64>,
    /// Fraction of active frames carrying real content just below the edge
    /// (how much evidence the ceiling tests are based on).
    pub hf_content: f64,
    /// Strongest edge of the side (L−R) channel.
    pub side_edge: Option<Edge>,
    /// Energy between 26 and 40 kHz relative to 2–20 kHz (dB), for sample
    /// rates above 48 kHz. Genuine high-resolution recordings typically show
    /// -40 to -80 dB; material upsampled from 44.1/48 kHz far less.
    pub ultrasonic_rel_db: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ArtifactMeasure {
    /// Evaluated frequency range (Hz).
    pub lo_hz: f64,
    pub hi_hz: f64,
    /// Frame-weighted mean spectral-hole ratio over the range.
    pub hole_ratio: Option<f64>,
    /// Highest mean hole ratio over any three adjacent evaluated sub-bands.
    pub top_hole_ratio: Option<f64>,
    /// Frame-weighted mean spectral flatness over the range.
    pub sfm: Option<f64>,
    pub frames: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SegmentBandwidth {
    pub start: f64,
    pub end: f64,
    /// Steep cutoff found in this segment.
    pub cutoff_hz: Option<f64>,
    pub edge_db: f64,
    /// Highest band within 50 dB of the segment's 1–8 kHz level.
    pub upper_hz: f64,
    /// 1–8 kHz reference level of the segment (dB), to tell breakdowns apart.
    pub reference_db: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BandwidthChange {
    pub time: f64,
    pub from_hz: f64,
    pub to_hz: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NoiseMeasure {
    /// Duration of quiet passages (-100 … -40 dBFS RMS) analysed.
    pub quiet_seconds: f64,
    /// Mean RMS level of those passages.
    pub quiet_rms_db: Option<f64>,
    /// Spectral flatness (0–1) of the quiet passages' average spectrum
    /// between 4 and 16 kHz: close to 1 for broadband noise (hiss).
    pub quiet_hf_flatness: Option<f64>,
}

fn prefix(v: &[f64]) -> Vec<f64> {
    let mut p = Vec::with_capacity(v.len() + 1);
    p.push(0.0);
    for x in v {
        p.push(p.last().unwrap() + x);
    }
    p
}

fn mean_range(prefix: &[f64], lo: usize, hi: usize) -> Option<f64> {
    // Inclusive lo, exclusive hi.
    if hi <= lo || hi > prefix.len() {
        return None;
    }
    Some((prefix[hi] - prefix[lo]) / (hi - lo) as f64)
}

/// Moving-average smoothing of a dB spectrum over ±`half_hz`.
pub fn smooth(ltas: &[f32], bin_hz: f64, half_hz: f64) -> Vec<f64> {
    let w = ((half_hz / bin_hz).round() as usize).max(1);
    let v: Vec<f64> = ltas.iter().map(|&x| x as f64).collect();
    let p = prefix(&v);
    (0..v.len())
        .map(|i| {
            let lo = i.saturating_sub(w);
            let hi = (i + w + 1).min(v.len());
            (p[hi] - p[lo]) / (hi - lo) as f64
        })
        .collect()
}

/// Finds the strongest falling edge of `raw` (dB per bin) in `[lo_hz, hi_hz]`.
pub fn find_edge(raw: &[f32], bin_hz: f64, lo_hz: f64, hi_hz: f64) -> Option<Edge> {
    if raw.len() < 16 {
        return None;
    }
    let ls = smooth(raw, bin_hz, 100.0);
    let last = ls.len() - 1;
    let b = |hz: f64| ((hz / bin_hz).round() as usize).min(last);
    let near = b(150.0).max(1);
    let far = b(1200.0).max(near + 2);
    let p = prefix(&ls);
    let lo = b(lo_hz).max(far);
    let hi = b(hi_hz).min(last.saturating_sub(near + 2));
    if hi <= lo {
        return None;
    }
    let mut best: Option<(usize, f64)> = None;
    for k in lo..=hi {
        let below = mean_range(&p, k - far, k - near + 1)?;
        let above = mean_range(&p, k + near, (k + far).min(last) + 1)?;
        let e = below - above;
        if best.is_none_or(|(_, be)| e > be) {
            best = Some((k, e));
        }
    }
    let (k, edge_db) = best?;
    if edge_db < 6.0 {
        return None;
    }
    let shelf_slice: Vec<f64> = ls[k.saturating_sub(b(2000.0))..k.saturating_sub(b(300.0)).max(k.saturating_sub(b(2000.0)) + 1)].to_vec();
    let floor_lo = (k + b(300.0)).min(last);
    let floor_hi = (k + b(3000.0)).min(last).max(floor_lo + 1).min(ls.len());
    let floor_slice: Vec<f64> = ls[floor_lo..floor_hi].to_vec();
    let shelf = median(&shelf_slice)?;
    let floor = median(&floor_slice).or_else(|| ls.get(floor_lo).copied())?;
    let drop = shelf - floor;
    // Halfway crossing.
    let mid_level = shelf - drop / 2.0;
    let scan_lo = k.saturating_sub(b(600.0));
    let scan_hi = (k + b(600.0)).min(last);
    let cross = (scan_lo..=scan_hi).find(|&i| ls[i] < mid_level).unwrap_or(k);
    // Transition width.
    let t_lo = (k.saturating_sub(b(1500.0))..=scan_hi).find(|&i| ls[i] < shelf - 3.0).unwrap_or(cross);
    let t_hi = (t_lo..=last).find(|&i| ls[i] < floor + 3.0).unwrap_or(last);
    // Floor flatness from the unsmoothed spectrum.
    let fl = (cross + b(300.0)).min(last);
    let fh = last.saturating_sub(b(100.0)).max(fl + 1).min(raw.len());
    let floor_vals: Vec<f64> = raw[fl..fh].iter().map(|&v| v as f64).collect();
    let stdev = if floor_vals.len() >= 2 {
        let m = floor_vals.iter().sum::<f64>() / floor_vals.len() as f64;
        (floor_vals.iter().map(|v| (v - m) * (v - m)).sum::<f64>() / floor_vals.len() as f64).sqrt()
    } else {
        0.0
    };
    Some(Edge {
        hz: cross as f64 * bin_hz,
        drop_db: drop,
        edge_db,
        shelf_db: shelf,
        floor_db: floor,
        transition_hz: (t_hi.saturating_sub(t_lo)) as f64 * bin_hz,
        floor_stdev_db: stdev,
    })
}

/// Highest frequency whose smoothed level is within 50 dB of the median
/// level between 1 and 8 kHz (the "music level"). This is the effective
/// bandwidth: the cutoff for lossy material, where content fades out for
/// naturally dark material, close to Nyquist for full-band material.
pub fn upper_bandwidth(ltas: &[f32], bin_hz: f64) -> f64 {
    if ltas.len() < 8 {
        return 0.0;
    }
    let ls = smooth(ltas, bin_hz, 100.0);
    let last = ls.len() - 1;
    let b = |hz: f64| ((hz / bin_hz).round() as usize).min(last);
    let (r0, r1) = (b(1000.0), b(8000.0).max(b(1000.0) + 1));
    let Some(reference) = median(&ls[r0..r1]) else { return 0.0 };
    let threshold = reference - 50.0;
    let start = b(20.0);
    let top = (start..=last).rev().find(|&i| ls[i] > threshold).unwrap_or(0);
    top as f64 * bin_hz
}

/// How consistently the energy stays below `cutoff_hz` over time.
fn ceiling_stats(hf: &HfFrames, cutoff_hz: f64, nyquist: f64, active_frames: u64) -> (Option<f64>, Option<f64>, f64) {
    let above_lo = hf.band_of_hz(cutoff_hz + 300.0);
    let below = (hf.band_of_hz((cutoff_hz - 2500.0).max(hf.start_hz)), hf.band_of_hz(cutoff_hz - 300.0));
    let (Some(a0), (Some(b0), Some(b1))) = (above_lo, below) else {
        return (None, None, 0.0);
    };
    let a1 = hf.band_of_hz(nyquist - 50.0).unwrap_or(hf.bands - 1).max(a0);
    if b1 < b0 {
        return (None, None, 0.0);
    }
    let mut diffs = Vec::new();
    let mut content_frames = 0u64;
    for f in 0..hf.frames() {
        let row = hf.row(f);
        let lin = |q: u8| 10f64.powf(q_to_db(q) / 10.0);
        let below_e: f64 = row[b0..=b1].iter().map(|&q| lin(q)).sum::<f64>() / (b1 - b0 + 1) as f64;
        let above_e: f64 = row[a0..=a1].iter().map(|&q| lin(q)).fold(0.0, f64::max);
        let below_db = 10.0 * below_e.max(1e-30).log10();
        let reference_db = q_to_db(hf.reference[f]) - 10.0 * ((b1 - b0 + 1) as f64).log10();
        // Count frames whose sub-cutoff region carries content within 50 dB
        // of the frame's 1–8 kHz reference level.
        if below_db > -125.0 && below_db > reference_db - 50.0 {
            content_frames += 1;
            diffs.push(10.0 * above_e.max(1e-30).log10() - below_db);
        }
    }
    let stored = hf.frames().max(1) as f64;
    let hf_content = (content_frames as f64 / stored).min(1.0);
    let _ = active_frames;
    if diffs.len() < 5 {
        return (None, None, hf_content);
    }
    let consistent = diffs.iter().filter(|&&d| d < -30.0).count() as f64 / diffs.len() as f64;
    diffs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    (Some(consistent), percentile_sorted(&diffs, 99.0), hf_content)
}

pub fn measure_bandwidth(spec: &SpectralResult) -> BandwidthMeasure {
    let nyq = spec.nyquist;
    let mut m = BandwidthMeasure { nyquist: nyq, ..Default::default() };
    if spec.active_frames == 0 || spec.ltas_db.is_empty() {
        return m;
    }
    m.upper_hz = upper_bandwidth(&spec.ltas_db, spec.bin_hz);
    m.edge = find_edge(&spec.ltas_db, spec.bin_hz, 4000.0, nyq - 50.0);
    if let Some(e) = &m.edge {
        let (c, p99, content) = ceiling_stats(&spec.hf, e.hz, nyq, spec.active_frames);
        m.ceiling_consistency = c;
        m.breakthrough_p99_db = p99;
        m.hf_content = content;
    }
    if let Some(side) = &spec.ltas_side_db {
        m.side_edge = find_edge(side, spec.bin_hz, 4000.0, nyq - 50.0);
    }
    if nyq > 24_000.0 {
        let b = |hz: f64| ((hz / spec.bin_hz) as usize).min(spec.ltas_db.len() - 1);
        let lin = |r: std::ops::Range<usize>| -> f64 {
            let n = r.len().max(1) as f64;
            spec.ltas_db[r].iter().map(|&d| 10f64.powf(d as f64 / 10.0)).sum::<f64>() / n
        };
        // Start above the transition band of a 44.1/48 kHz source's
        // anti-aliasing or resampling filter.
        let hi = lin(b(26_000.0)..b(nyq.min(40_000.0)));
        let mid = lin(b(2_000.0)..b(20_000.0));
        if mid > 0.0 {
            m.ultrasonic_rel_db = Some(10.0 * (hi.max(1e-30) / mid).log10());
        }
    }
    m
}

/// Spectral-hole statistics in the region below the cutoff (or the upper
/// bandwidth when there is no steep cutoff). Sub-bands within `margin_hz` of
/// the cutoff are excluded: a band that straddles a steep slope always
/// contains bins far below its median, whatever the cause of the slope.
pub fn measure_artifacts(subbands: &[SubbandStats], top_hz: f64, margin_hz: f64, nyquist: f64) -> ArtifactMeasure {
    let hi_hz = (top_hz - margin_hz).min(nyquist - 1000.0);
    let lo_hz = (hi_hz - 7000.0).max(5000.0);
    let max_frames = subbands.iter().map(|s| s.frames).max().unwrap_or(0);
    let selected: Vec<&SubbandStats> =
        subbands.iter().filter(|s| s.lo_hz >= lo_hz - 1.0 && s.hi_hz <= hi_hz + 1.0 && s.frames > 0 && s.frames * 10 >= max_frames).collect();
    let mut out = ArtifactMeasure { lo_hz, hi_hz, ..Default::default() };
    if selected.is_empty() || hi_hz <= lo_hz {
        return out;
    }
    let wmean = |f: &dyn Fn(&SubbandStats) -> f64, set: &[&SubbandStats]| -> f64 {
        let t: u64 = set.iter().map(|s| s.frames).sum();
        set.iter().map(|s| f(s) * s.frames as f64).sum::<f64>() / t.max(1) as f64
    };
    out.hole_ratio = Some(wmean(&|s| s.mean_hole_ratio, &selected));
    out.sfm = Some(wmean(&|s| s.mean_sfm, &selected));
    // Strongest region: best mean over any three adjacent selected sub-bands.
    let mut best: Option<f64> = None;
    for w in selected.windows(3.min(selected.len())) {
        let v = wmean(&|s| s.mean_hole_ratio, w);
        best = Some(best.map_or(v, |b: f64| b.max(v)));
    }
    out.top_hole_ratio = best;
    out.frames = selected.iter().map(|s| s.frames).sum::<u64>() / selected.len() as u64;
    out
}

/// Per-segment cutoffs from the stored high-frequency band frames.
pub fn measure_segments(hf: &HfFrames, segment_secs: f64, duration: f64) -> Vec<SegmentBandwidth> {
    let mut out = Vec::new();
    if hf.frames() == 0 || hf.bands < 12 {
        return out;
    }
    let nseg = (duration / segment_secs).ceil().max(1.0) as usize;
    let mut f = 0usize;
    for s in 0..nseg {
        let (t0, t1) = (s as f64 * segment_secs, ((s + 1) as f64 * segment_secs).min(duration));
        let mut acc = vec![0f64; hf.bands];
        let mut refs = Vec::new();
        let mut n = 0usize;
        while f < hf.frames() && (hf.times[f] as f64) < t1 {
            if hf.times[f] as f64 >= t0 {
                for (a, &q) in acc.iter_mut().zip(hf.row(f)) {
                    *a += 10f64.powf(q_to_db(q) / 10.0);
                }
                refs.push(q_to_db(hf.reference[f]));
                n += 1;
            }
            f += 1;
        }
        if n < 3 {
            continue;
        }
        let db: Vec<f64> = acc.iter().map(|&a| 10.0 * (a / n as f64).max(1e-30).log10()).collect();
        let mut best: Option<(usize, f64)> = None;
        for b in 4..db.len().saturating_sub(4) {
            let below = db[b - 4..b].iter().sum::<f64>() / 4.0;
            let above = db[b + 1..b + 5].iter().sum::<f64>() / 4.0;
            let e = below - above;
            if best.is_none_or(|(_, be)| e > be) {
                best = Some((b, e));
            }
        }
        let reference_db = median(&refs).unwrap_or(-200.0);
        // Upper extent: highest band within 50 dB of the segment's 1–8 kHz
        // band level (same rule as the whole-file upper bandwidth).
        let ref_bands: Vec<f64> = (0..db.len()).filter(|&b| hf.band_center(b) <= 8000.0).map(|b| db[b]).collect();
        let level = median(&ref_bands).unwrap_or(-200.0);
        let upper_hz = db.iter().rposition(|&v| v > level - 50.0).map(|b| hf.start_hz + (b as f64 + 1.0) * hf.band_hz).unwrap_or(hf.start_hz);
        let (cutoff_hz, edge_db) = match best {
            Some((b, e)) if e >= 25.0 => (Some(hf.start_hz + b as f64 * hf.band_hz + hf.band_hz / 2.0), e),
            Some((_, e)) => (None, e),
            None => (None, 0.0),
        };
        out.push(SegmentBandwidth { start: t0, end: t1, cutoff_hz, edge_db, upper_hz, reference_db });
    }
    out
}

/// Abrupt changes of the steep cutoff between adjacent comparable segments.
pub fn bandwidth_changes(segments: &[SegmentBandwidth]) -> Vec<BandwidthChange> {
    let refs: Vec<f64> = segments.iter().map(|s| s.reference_db).collect();
    let Some(typical) = median(&refs) else { return Vec::new() };
    let comparable = |s: &SegmentBandwidth| s.reference_db > typical - 10.0;
    let mut out = Vec::new();
    for w in segments.windows(2) {
        let (a, b) = (&w[0], &w[1]);
        if !(comparable(a) && comparable(b)) {
            continue;
        }
        // Effective extent: the steep cutoff if there is one, else the upper bandwidth.
        let fa = a.cutoff_hz.unwrap_or(a.upper_hz);
        let fb = b.cutoff_hz.unwrap_or(b.upper_hz);
        let (low, high) = if fa < fb { (a, fa.max(fb)) } else { (b, fa.max(fb)) };
        let Some(low_cut) = low.cutoff_hz else { continue };
        if high - low_cut >= 1500.0 && low.edge_db >= 30.0 && (11_000.0..20_500.0).contains(&low_cut) {
            out.push(BandwidthChange { time: b.start, from_hz: fa, to_hz: fb });
        }
    }
    out
}

pub fn measure_noise(spec: &SpectralResult) -> NoiseMeasure {
    let quiet_seconds = spec.quiet_frames as f64 * spec.frame_secs;
    let mut out = NoiseMeasure { quiet_seconds, quiet_rms_db: spec.quiet_rms_db, quiet_hf_flatness: None };
    if let Some(q) = &spec.quiet_ltas_db {
        let lo = (4000.0 / spec.bin_hz) as usize;
        let hi = ((16000.0f64.min(spec.nyquist - 500.0)) / spec.bin_hz) as usize;
        if hi > lo + 16 && quiet_seconds >= 1.0 {
            let lin: Vec<f64> = q[lo..hi].iter().map(|&d| 10f64.powf(d as f64 / 10.0).max(1e-30)).collect();
            let am = lin.iter().sum::<f64>() / lin.len() as f64;
            let gm = (lin.iter().map(|v| v.ln()).sum::<f64>() / lin.len() as f64).exp();
            out.quiet_hf_flatness = Some((gm / am).clamp(0.0, 1.0));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_ltas(bin_hz: f64, bins: usize, cutoff: f64, slope_db_per_khz: f64, floor: f32) -> Vec<f32> {
        (0..bins)
            .map(|i| {
                let f = i as f64 * bin_hz;
                let base = -40.0 - 1.5 * (f / 1000.0); // gentle natural tilt
                if f < cutoff {
                    base as f32
                } else {
                    (base - slope_db_per_khz * (f - cutoff) / 1000.0).max(floor as f64) as f32
                }
            })
            .collect()
    }

    #[test]
    fn brickwall_edge_is_found_and_steep() {
        let bin_hz = 44_100.0 / 4096.0;
        let ltas = synthetic_ltas(bin_hz, 2049, 16_000.0, 400.0, -130.0);
        let e = find_edge(&ltas, bin_hz, 4000.0, 22_000.0).unwrap();
        assert!((e.hz - 16_000.0).abs() < 250.0, "{}", e.hz);
        assert!(e.is_steep(), "{e:?}");
        assert!(e.drop_db > 50.0);
    }

    #[test]
    fn gentle_rolloff_is_not_steep() {
        let bin_hz = 44_100.0 / 4096.0;
        // 6 dB per kHz roll-off above 12 kHz (a dark master).
        let ltas = synthetic_ltas(bin_hz, 2049, 12_000.0, 6.0, -130.0);
        let e = find_edge(&ltas, bin_hz, 4000.0, 22_000.0);
        assert!(e.as_ref().is_none_or(|e| !e.is_steep()), "{e:?}");
    }

    #[test]
    fn segments_detect_spliced_bandwidth() {
        // 30 s at full band then 30 s with a 16 kHz ceiling.
        let band_hz = 250.0;
        let bands = 84;
        let mut hf = HfFrames { start_hz: 1000.0, band_hz, bands, ..Default::default() };
        for f in 0..600 {
            let t = f as f64 * 0.1;
            let cutoff = if t < 30.0 { 21_500.0 } else { 16_000.0 };
            for b in 0..bands {
                let hz = 1000.0 + (b as f64 + 0.5) * band_hz;
                let db = if hz < cutoff { -50.0 - hz / 2000.0 } else { -140.0 };
                hf.data.push(crate::analysis::spectral::db_to_q(db));
            }
            hf.reference.push(crate::analysis::spectral::db_to_q(-30.0));
            hf.times.push(t as f32);
        }
        let segs = measure_segments(&hf, 10.0, 60.0);
        assert_eq!(segs.len(), 6);
        let changes = bandwidth_changes(&segs);
        assert_eq!(changes.len(), 1, "{segs:?}");
        assert!((changes[0].time - 30.0).abs() < 1e-6);
        assert!((changes[0].to_hz - 16_000.0).abs() < 300.0);
    }
}
