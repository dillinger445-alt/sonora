//! Acoustic fingerprint comparison.
//!
//! Fingerprints are sequences of 32-bit sub-fingerprints at 10 frames per
//! second (see [`super::spectral`]): bit *m* records the sign of the change,
//! from one frame to the next, of the log-energy difference between bands
//! *m* and *m+1* (33 log-spaced bands between 300 Hz and 3 kHz), following
//! the approach of Haitsma & Kalker (2002). The value 0 marks silent frames.
//!
//! Two encodings of the same recording typically differ in 5–20 % of bits;
//! unrelated audio differs in ≈ 50 %.

use super::spectral::FP_RATE;

/// Local window used to find matching segments (5 s).
const WINDOW: usize = 50;
/// A window "matches" below this bit error rate. Re-encodes of the same
/// recording measure 0.0–0.10; unrelated music 0.3–0.5 (similar-sounding
/// productions can dip towards 0.3 in places, hence the margin).
pub const MATCH_BER: f64 = 0.20;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Comparison {
    /// Offset of `b` relative to `a`, in frames (b[i] ↔ a[i + offset]).
    pub offset: i64,
    /// Bit error rate over all non-silent overlapping frames.
    pub ber: f64,
    /// Overlapping non-silent frames compared.
    pub overlap: usize,
    /// Frames inside matching windows.
    pub matched: usize,
    /// Longest contiguous matching stretch, in frames.
    pub longest_run: usize,
}

impl Comparison {
    pub fn matched_secs(&self) -> f64 {
        self.matched as f64 / FP_RATE
    }

    pub fn longest_run_secs(&self) -> f64 {
        self.longest_run as f64 / FP_RATE
    }

    pub fn offset_secs(&self) -> f64 {
        self.offset as f64 / FP_RATE
    }
}

/// Compares `b` against `a` at a fixed frame offset.
pub fn compare_at(a: &[u32], b: &[u32], offset: i64) -> Option<Comparison> {
    let start_b = if offset < 0 { (-offset) as usize } else { 0 };
    let start_a = if offset > 0 { offset as usize } else { 0 };
    if start_a >= a.len() || start_b >= b.len() {
        return None;
    }
    let len = (a.len() - start_a).min(b.len() - start_b);
    let (sa, sb) = (&a[start_a..start_a + len], &b[start_b..start_b + len]);
    let mut errors = 0u64;
    let mut compared = 0usize;
    let mut matched = 0usize;
    let mut run = 0usize;
    let mut longest = 0usize;
    for (wa, wb) in sa.chunks(WINDOW).zip(sb.chunks(WINDOW)) {
        let mut we = 0u32;
        let mut wn = 0usize;
        for (&x, &y) in wa.iter().zip(wb.iter()) {
            if x == 0 || y == 0 {
                continue;
            }
            we += (x ^ y).count_ones();
            wn += 1;
        }
        errors += we as u64;
        compared += wn;
        // Require most of the window to be audible before judging it.
        if wn * 2 >= wa.len() && (we as f64 / (wn as f64 * 32.0)) < MATCH_BER {
            matched += wn;
            run += wn;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    if compared == 0 {
        return None;
    }
    Some(Comparison { offset, ber: errors as f64 / (compared as f64 * 32.0), overlap: compared, matched, longest_run: longest })
}

/// Searches offsets `center - radius ..= center + radius` and returns the
/// comparison with the most matched frames (ties broken by lower BER).
pub fn best_near(a: &[u32], b: &[u32], center: i64, radius: i64) -> Option<Comparison> {
    let mut best: Option<Comparison> = None;
    for off in center - radius..=center + radius {
        if let Some(c) = compare_at(a, b, off) {
            let better = match &best {
                None => true,
                Some(bc) => c.matched > bc.matched || (c.matched == bc.matched && c.ber < bc.ber),
            };
            if better {
                best = Some(c);
            }
        }
    }
    best
}

/// Lookup keys for candidate search: the middle 16 bits of every `step`-th
/// audible sub-fingerprint, with its frame position.
pub fn keys(fp: &[u32], step: usize) -> Vec<(u16, u32)> {
    fp.iter()
        .enumerate()
        .step_by(step.max(1))
        .filter(|(_, &v)| v != 0)
        .map(|(i, &v)| (((v >> 8) & 0xFFFF) as u16, i as u32))
        .collect()
}

/// Serialises a fingerprint as little-endian bytes (for storage).
pub fn to_bytes(fp: &[u32]) -> Vec<u8> {
    fp.iter().flat_map(|v| v.to_le_bytes()).collect()
}

pub fn from_bytes(b: &[u8]) -> Vec<u32> {
    b.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pseudo(seed: u64, n: usize) -> Vec<u32> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                ((s >> 32) as u32) | 1
            })
            .collect()
    }

    fn flip_bits(v: &[u32], every: u64) -> Vec<u32> {
        let mut s = 99u64;
        v.iter()
            .map(|&x| {
                let mut y = x;
                for bit in 0..32 {
                    s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                    if (s >> 33).is_multiple_of(every) {
                        y ^= 1 << bit;
                    }
                }
                if y == 0 {
                    1
                } else {
                    y
                }
            })
            .collect()
    }

    #[test]
    fn identical_and_noisy_copies_match() {
        let a = pseudo(1, 3000);
        let same = compare_at(&a, &a, 0).unwrap();
        assert_eq!(same.ber, 0.0);
        assert_eq!(same.matched, 3000);
        let noisy = flip_bits(&a, 10); // ~10 % bit errors
        let c = compare_at(&a, &noisy, 0).unwrap();
        assert!(c.ber > 0.05 && c.ber < 0.15, "{}", c.ber);
        assert!(c.matched as f64 > 0.95 * 3000.0);
        let unrelated = pseudo(2, 3000);
        let u = compare_at(&a, &unrelated, 0).unwrap();
        assert!((u.ber - 0.5).abs() < 0.03);
        assert_eq!(u.matched, 0);
    }

    #[test]
    fn finds_offset_and_partial_overlap() {
        let a = pseudo(5, 4000);
        // b = an "edit": 60 s of a starting at 100 s, preceded by 20 s of unrelated audio.
        let mut b = pseudo(77, 200);
        b.extend_from_slice(&a[1000..1600]);
        let best = best_near(&a, &b, 800, 5).unwrap();
        assert_eq!(best.offset, 800);
        assert!((best.longest_run_secs() - 60.0).abs() <= 5.0, "{}", best.longest_run_secs());
        let k = keys(&a, 8);
        assert_eq!(k.len(), 500);
        assert_eq!(from_bytes(&to_bytes(&a)), a);
    }
}
