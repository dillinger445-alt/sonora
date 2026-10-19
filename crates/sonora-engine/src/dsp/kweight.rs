//! ITU-R BS.1770-4 K-weighting filter.
//!
//! The recommendation specifies coefficients only for 48 kHz. For other
//! sample rates the two stages are re-derived from their analog prototypes
//! with the bilinear transform (the same parametrisation as libebur128), which
//! reproduces the published 48 kHz coefficients to within 1e-6.

use super::biquad::BiquadCoef;
use std::f64::consts::PI;

/// Stage 1: high-shelf "pre-filter" modelling the acoustic effect of the head.
pub fn pre_filter(fs: f64) -> BiquadCoef {
    let f0 = 1681.974450955533;
    let g = 3.999843853973347;
    let q = 0.7071752369554196;
    let k = (PI * f0 / fs).tan();
    let vh = 10f64.powf(g / 20.0);
    let vb = vh.powf(0.4996667741545416);
    let a0 = 1.0 + k / q + k * k;
    BiquadCoef {
        b0: (vh + vb * k / q + k * k) / a0,
        b1: 2.0 * (k * k - vh) / a0,
        b2: (vh - vb * k / q + k * k) / a0,
        a1: 2.0 * (k * k - 1.0) / a0,
        a2: (1.0 - k / q + k * k) / a0,
    }
}

/// Stage 2: "RLB" high-pass. The numerator is the unnormalised [1, -2, 1]
/// used by the recommendation.
pub fn rlb_filter(fs: f64) -> BiquadCoef {
    let f0 = 38.13547087602444;
    let q = 0.5003270373238773;
    let k = (PI * f0 / fs).tan();
    let a0 = 1.0 + k / q + k * k;
    BiquadCoef { b0: 1.0, b1: -2.0, b2: 1.0, a1: 2.0 * (k * k - 1.0) / a0, a2: (1.0 - k / q + k * k) / a0 }
}

/// BS.1770 channel weight for channel `index` of a `channels`-channel stream.
/// Front channels weigh 1.0, surround channels 1.41 (+1.5 dB) and the LFE
/// channel of a 5.1 layout is excluded.
pub fn channel_weight(index: usize, channels: usize) -> f64 {
    if channels == 6 {
        match index {
            3 => 0.0,
            4 | 5 => 1.41,
            _ => 1.0,
        }
    } else if channels == 5 {
        match index {
            3 | 4 => 1.41,
            _ => 1.0,
        }
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_published_48k_coefficients() {
        let p = pre_filter(48_000.0);
        let expect = [1.53512485958697, -2.69169618940638, 1.19839281085285, -1.69065929318241, 0.73248077421585];
        for (a, b) in [p.b0, p.b1, p.b2, p.a1, p.a2].iter().zip(expect.iter()) {
            assert!((a - b).abs() < 1e-6, "pre-filter {a} vs {b}");
        }
        let r = rlb_filter(48_000.0);
        assert!((r.a1 - -1.99004745483398).abs() < 1e-6);
        assert!((r.a2 - 0.99007225036621).abs() < 1e-6);
    }

    #[test]
    fn response_shape() {
        for fs in [44_100.0, 48_000.0, 96_000.0] {
            let p = pre_filter(fs);
            let r = rlb_filter(fs);
            let gain_db = |f: f64| 20.0 * (p.magnitude(fs, f) * r.magnitude(fs, f)).log10();
            // ~+4 dB shelf at high frequencies, ~0 dB around 500 Hz–1 kHz, steep low cut.
            assert!((gain_db(10_000.0) - 4.0).abs() < 0.3, "fs {fs}: {}", gain_db(10_000.0));
            assert!(gain_db(500.0).abs() < 0.3, "fs {fs}: {}", gain_db(500.0));
            assert!(gain_db(20.0) < -10.0);
        }
    }
}
