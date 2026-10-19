//! Second-order IIR sections (transposed direct form II, double precision).

use std::f64::consts::PI;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BiquadCoef {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct BiquadState {
    z1: f64,
    z2: f64,
}

impl BiquadState {
    #[inline(always)]
    pub fn process(&mut self, c: &BiquadCoef, x: f64) -> f64 {
        let y = c.b0 * x + self.z1;
        self.z1 = c.b1 * x - c.a1 * y + self.z2;
        self.z2 = c.b2 * x - c.a2 * y;
        y
    }
}

impl BiquadCoef {
    /// RBJ cookbook low-pass.
    pub fn lowpass(fs: f64, f0: f64, q: f64) -> BiquadCoef {
        let w = 2.0 * PI * f0 / fs;
        let (sw, cw) = w.sin_cos();
        let alpha = sw / (2.0 * q);
        let a0 = 1.0 + alpha;
        BiquadCoef {
            b0: (1.0 - cw) / 2.0 / a0,
            b1: (1.0 - cw) / a0,
            b2: (1.0 - cw) / 2.0 / a0,
            a1: -2.0 * cw / a0,
            a2: (1.0 - alpha) / a0,
        }
    }

    /// RBJ cookbook high-pass.
    pub fn highpass(fs: f64, f0: f64, q: f64) -> BiquadCoef {
        let w = 2.0 * PI * f0 / fs;
        let (sw, cw) = w.sin_cos();
        let alpha = sw / (2.0 * q);
        let a0 = 1.0 + alpha;
        BiquadCoef {
            b0: (1.0 + cw) / 2.0 / a0,
            b1: -(1.0 + cw) / a0,
            b2: (1.0 + cw) / 2.0 / a0,
            a1: -2.0 * cw / a0,
            a2: (1.0 - alpha) / a0,
        }
    }

    /// Magnitude response (linear) at frequency `f`.
    pub fn magnitude(&self, fs: f64, f: f64) -> f64 {
        let w = 2.0 * PI * f / fs;
        let (s1, c1) = w.sin_cos();
        let (s2, c2) = (2.0 * w).sin_cos();
        let nr = self.b0 + self.b1 * c1 + self.b2 * c2;
        let ni = -(self.b1 * s1 + self.b2 * s2);
        let dr = 1.0 + self.a1 * c1 + self.a2 * c2;
        let di = -(self.a1 * s1 + self.a2 * s2);
        ((nr * nr + ni * ni) / (dr * dr + di * di)).sqrt()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowpass_response() {
        let c = BiquadCoef::lowpass(48_000.0, 1000.0, std::f64::consts::FRAC_1_SQRT_2);
        assert!((c.magnitude(48_000.0, 10.0) - 1.0).abs() < 1e-3);
        let at_fc = 20.0 * c.magnitude(48_000.0, 1000.0).log10();
        assert!((at_fc + 3.01).abs() < 0.05);
        assert!(c.magnitude(48_000.0, 10_000.0) < 0.02);
    }
}
