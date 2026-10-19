//! Small numeric helpers shared by the DSP and analysis modules.

/// Smallest linear amplitude considered for dB conversion (−200 dBFS).
pub const MIN_AMP: f64 = 1e-10;

/// Converts a linear amplitude to dBFS.
#[inline]
pub fn amp_to_db(x: f64) -> f64 {
    20.0 * x.abs().max(MIN_AMP).log10()
}

/// Converts a power (mean-square) value to dB.
#[inline]
pub fn pow_to_db(p: f64) -> f64 {
    10.0 * p.max(1e-20).log10()
}

#[inline]
pub fn db_to_amp(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

#[inline]
pub fn db_to_pow(db: f64) -> f64 {
    10f64.powf(db / 10.0)
}

#[inline]
pub fn clamp01(x: f64) -> f64 {
    if x.is_nan() {
        0.0
    } else {
        x.clamp(0.0, 1.0)
    }
}

/// Linear interpolation of `x` from `[x0, x1]` onto `[0, 1]`, clamped.
#[inline]
pub fn ramp(x: f64, x0: f64, x1: f64) -> f64 {
    if x1 == x0 {
        return if x >= x1 { 1.0 } else { 0.0 };
    }
    clamp01((x - x0) / (x1 - x0))
}

/// Percentile (0–100) of an already sorted slice using linear interpolation.
pub fn percentile_sorted(sorted: &[f64], pct: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    if sorted.len() == 1 {
        return Some(sorted[0]);
    }
    let rank = (pct.clamp(0.0, 100.0) / 100.0) * (sorted.len() - 1) as f64;
    let lo = rank.floor() as usize;
    let hi = rank.ceil() as usize;
    let frac = rank - lo as f64;
    Some(sorted[lo] + (sorted[hi] - sorted[lo]) * frac)
}

/// Percentile of an unsorted slice (copies and sorts).
pub fn percentile(values: &[f64], pct: f64) -> Option<f64> {
    let mut v: Vec<f64> = values.iter().copied().filter(|x| x.is_finite()).collect();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    percentile_sorted(&v, pct)
}

pub fn median(values: &[f64]) -> Option<f64> {
    percentile(values, 50.0)
}

/// Rounds to `decimals` places.
#[inline]
pub fn round_to(x: f64, decimals: u32) -> f64 {
    let p = 10f64.powi(decimals as i32);
    (x * p).round() / p
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding.
pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Decodes standard base64 (padding optional). Returns `None` on invalid input.
pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    }
    let bytes: Vec<u8> = s.bytes().filter(|c| !c.is_ascii_whitespace()).collect();
    let trimmed: &[u8] = {
        let mut end = bytes.len();
        while end > 0 && bytes[end - 1] == b'=' {
            end -= 1;
        }
        &bytes[..end]
    };
    let mut out = Vec::with_capacity(trimmed.len() * 3 / 4);
    for chunk in trimmed.chunks(4) {
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            n |= val(c)? << (18 - 6 * i);
        }
        match chunk.len() {
            4 => {
                out.push((n >> 16) as u8);
                out.push((n >> 8) as u8);
                out.push(n as u8);
            }
            3 => {
                out.push((n >> 16) as u8);
                out.push((n >> 8) as u8);
            }
            2 => out.push((n >> 16) as u8),
            _ => return None,
        }
    }
    Some(out)
}

/// Formats seconds as `m:ss` (or `h:mm:ss`).
pub fn fmt_time(secs: f64) -> String {
    let t = secs.max(0.0).round() as u64;
    let (h, m, s) = (t / 3600, (t / 60) % 60, t % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Formats a frequency in Hz as a compact human string (e.g. `16.1 kHz`).
pub fn fmt_hz(hz: f64) -> String {
    if hz >= 1000.0 {
        format!("{:.1} kHz", hz / 1000.0)
    } else {
        format!("{hz:.0} Hz")
    }
}

/// A bounded sample of event times spread over a whole stream. Once full,
/// every other entry is dropped and the minimum spacing doubles, so events
/// late in a long file are represented as well as early ones (instead of
/// keeping only the first N). Each entry carries a small tag.
#[derive(Clone, Debug)]
pub struct SpreadTimes {
    items: Vec<(f64, u32)>,
    cap: usize,
    min_gap: f64,
}

impl SpreadTimes {
    pub fn new(cap: usize) -> Self {
        SpreadTimes { items: Vec::new(), cap: cap.max(2), min_gap: 0.0 }
    }

    pub fn push(&mut self, time: f64, tag: u32) {
        if let Some(&(last, _)) = self.items.last() {
            if time - last < self.min_gap {
                return;
            }
        }
        self.items.push((time, tag));
        if self.items.len() > self.cap {
            let span = self.items.last().map_or(0.0, |l| l.0) - self.items[0].0;
            let mut i = 0usize;
            self.items.retain(|_| {
                i += 1;
                i % 2 == 1
            });
            self.min_gap = (self.min_gap * 2.0).max(span / self.cap as f64);
        }
    }

    pub fn retain(&mut self, f: impl FnMut(&(f64, u32)) -> bool) {
        self.items.retain(f);
    }

    pub fn items(&self) -> &[(f64, u32)] {
        &self.items
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// Evenly thins a sorted list to at most `cap` entries (keeps first and last).
pub fn thin_evenly(v: Vec<f64>, cap: usize) -> Vec<f64> {
    if v.len() <= cap || cap < 2 {
        return v;
    }
    let n = v.len();
    (0..cap).map(|i| v[i * (n - 1) / (cap - 1)]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_roundtrip() {
        assert!((amp_to_db(1.0)).abs() < 1e-12);
        assert!((amp_to_db(0.5) + 6.0206).abs() < 1e-3);
        assert!((db_to_amp(-6.0206) - 0.5).abs() < 1e-4);
    }

    #[test]
    fn percentiles() {
        let v = [5.0, 1.0, 3.0, 2.0, 4.0];
        assert_eq!(percentile(&v, 50.0), Some(3.0));
        assert_eq!(percentile(&v, 0.0), Some(1.0));
        assert_eq!(percentile(&v, 100.0), Some(5.0));
        assert_eq!(percentile(&v, 25.0), Some(2.0));
        assert_eq!(percentile(&[], 50.0), None);
    }

    #[test]
    fn base64() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        for s in ["", "f", "fo", "foo", "foob", "fooba", "foobar"] {
            assert_eq!(base64_decode(&base64_encode(s.as_bytes())).unwrap(), s.as_bytes());
        }
        assert!(base64_decode("@@@@").is_none());
    }

    #[test]
    fn time_and_hz() {
        assert_eq!(fmt_time(65.4), "1:05");
        assert_eq!(fmt_time(3725.0), "1:02:05");
        assert_eq!(fmt_hz(16_050.0), "16.1 kHz");
        assert_eq!(fmt_hz(440.0), "440 Hz");
    }

    #[test]
    fn spread_times_cover_the_whole_stream() {
        let mut st = SpreadTimes::new(100);
        for i in 0..100_000 {
            st.push(i as f64 * 0.01, 0);
        }
        assert!(st.len() <= 100 && st.len() >= 50, "{}", st.len());
        let first = st.items()[0].0;
        let last = st.items()[st.len() - 1].0;
        assert!(first < 1.0, "{first}");
        assert!(last > 900.0, "events late in the stream are kept: {last}");
        let v = thin_evenly((0..1000).map(|i| i as f64).collect(), 10);
        assert_eq!(v.len(), 10);
        assert_eq!(v[0], 0.0);
        assert_eq!(v[9], 999.0);
    }
}
