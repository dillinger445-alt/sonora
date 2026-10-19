//! Radix-2 FFT (complex) and a real-input FFT built on a half-size complex
//! transform. Single precision is ample for audio analysis: the numerical
//! noise floor sits around −130 dB relative to full scale.
//!
//! Twiddle factors are stored per stage, contiguously, and each butterfly
//! stage works on split slices, which lets the compiler vectorise the inner
//! loop without bounds checks.

use std::f64::consts::PI;

/// In-place iterative radix-2 complex FFT of a fixed power-of-two size.
pub struct Fft {
    n: usize,
    rev: Vec<u32>,
    /// Twiddles of all stages: the stage with half-size `h` occupies
    /// `[h - 1, 2h - 1)`.
    tw_re: Vec<f32>,
    tw_im: Vec<f32>,
}

impl Fft {
    pub fn new(n: usize) -> Fft {
        assert!(n.is_power_of_two() && n >= 2, "FFT size must be a power of two");
        let bits = n.trailing_zeros();
        let rev = (0..n as u32).map(|i| i.reverse_bits() >> (32 - bits)).collect();
        let mut tw_re = Vec::with_capacity(n);
        let mut tw_im = Vec::with_capacity(n);
        let mut half = 1;
        while half < n {
            for k in 0..half {
                let a = -PI * k as f64 / half as f64;
                tw_re.push(a.cos() as f32);
                tw_im.push(a.sin() as f32);
            }
            half *= 2;
        }
        Fft { n, rev, tw_re, tw_im }
    }

    pub fn len(&self) -> usize {
        self.n
    }

    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    /// Forward transform (no scaling).
    pub fn forward(&self, re: &mut [f32], im: &mut [f32]) {
        let n = self.n;
        assert!(re.len() == n && im.len() == n);
        for i in 0..n {
            let j = self.rev[i] as usize;
            if j > i {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        // First stage: twiddle 1.
        for (r, i) in re.as_chunks_mut::<2>().0.iter_mut().zip(im.as_chunks_mut::<2>().0.iter_mut()) {
            let (a, b) = (r[0], r[1]);
            r[0] = a + b;
            r[1] = a - b;
            let (c, d) = (i[0], i[1]);
            i[0] = c + d;
            i[1] = c - d;
        }
        let mut half = 2;
        while half < n {
            let size = 2 * half;
            let twr = &self.tw_re[half - 1..2 * half - 1];
            let twi = &self.tw_im[half - 1..2 * half - 1];
            for (rb, ib) in re.chunks_exact_mut(size).zip(im.chunks_exact_mut(size)) {
                let (rl, rh) = rb.split_at_mut(half);
                let (il, ih) = ib.split_at_mut(half);
                // Equal-length slices let the optimiser drop bounds checks
                // and vectorise the loop.
                let (rl, rh, il, ih) = (&mut rl[..half], &mut rh[..half], &mut il[..half], &mut ih[..half]);
                let (twr, twi) = (&twr[..half], &twi[..half]);
                for k in 0..half {
                    let (wr, wi) = (twr[k], twi[k]);
                    let (hr, hi) = (rh[k], ih[k]);
                    let xr = hr * wr - hi * wi;
                    let xi = hr * wi + hi * wr;
                    let (lr, li) = (rl[k], il[k]);
                    rh[k] = lr - xr;
                    ih[k] = li - xi;
                    rl[k] = lr + xr;
                    il[k] = li + xi;
                }
            }
            half = size;
        }
    }
}

/// Real-input FFT producing `n/2 + 1` bins.
pub struct RealFft {
    n: usize,
    half: Fft,
    w_re: Vec<f32>,
    w_im: Vec<f32>,
    re: Vec<f32>,
    im: Vec<f32>,
}

impl RealFft {
    pub fn new(n: usize) -> RealFft {
        assert!(n >= 4 && n.is_power_of_two());
        let mut w_re = Vec::with_capacity(n / 2 + 1);
        let mut w_im = Vec::with_capacity(n / 2 + 1);
        for k in 0..=n / 2 {
            let a = -2.0 * PI * k as f64 / n as f64;
            w_re.push(a.cos() as f32);
            w_im.push(a.sin() as f32);
        }
        RealFft { n, half: Fft::new(n / 2), w_re, w_im, re: vec![0.0; n / 2], im: vec![0.0; n / 2] }
    }

    pub fn len(&self) -> usize {
        self.n
    }

    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    pub fn bins(&self) -> usize {
        self.n / 2 + 1
    }

    /// Computes the complex spectrum of `input` (length `n`) into `out_re`/`out_im` (length `n/2+1`).
    pub fn forward(&mut self, input: &[f32], out_re: &mut [f32], out_im: &mut [f32]) {
        let h = self.n / 2;
        for ((r, i), pair) in self.re.iter_mut().zip(self.im.iter_mut()).zip(input.as_chunks::<2>().0) {
            *r = pair[0];
            *i = pair[1];
        }
        self.half.forward(&mut self.re, &mut self.im);
        for k in 0..=h {
            let a = k % h;
            let b = (h - k) % h;
            let (zr, zi) = (self.re[a], self.im[a]);
            let (cr, ci) = (self.re[b], -self.im[b]); // conj(Z[h-k])
            let er = 0.5 * (zr + cr);
            let ei = 0.5 * (zi + ci);
            // O = -i (z - c) / 2
            let or = 0.5 * (zi - ci);
            let oi = -0.5 * (zr - cr);
            let (wr, wi) = (self.w_re[k], self.w_im[k]);
            out_re[k] = er + (or * wr - oi * wi);
            out_im[k] = ei + (or * wi + oi * wr);
        }
    }

    /// Computes the power spectrum |X[k]|² of `input` into `out` (length `n/2+1`).
    pub fn power(&mut self, input: &[f32], out: &mut [f32], scratch_re: &mut Vec<f32>, scratch_im: &mut Vec<f32>) {
        let bins = self.bins();
        scratch_re.resize(bins, 0.0);
        scratch_im.resize(bins, 0.0);
        self.forward(input, scratch_re, scratch_im);
        for k in 0..bins {
            out[k] = scratch_re[k] * scratch_re[k] + scratch_im[k] * scratch_im[k];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn naive_dft(x: &[f32]) -> Vec<(f64, f64)> {
        let n = x.len();
        (0..=n / 2)
            .map(|k| {
                let mut re = 0.0;
                let mut im = 0.0;
                for (t, &v) in x.iter().enumerate() {
                    let a = -2.0 * PI * (k * t) as f64 / n as f64;
                    re += v as f64 * a.cos();
                    im += v as f64 * a.sin();
                }
                (re, im)
            })
            .collect()
    }

    #[test]
    fn real_fft_matches_dft() {
        for n in [4usize, 8, 64, 512, 2048] {
            let x: Vec<f32> = (0..n).map(|i| ((i * 7919 % 113) as f32 / 56.0 - 1.0) * 0.7 + (i as f32 * 0.3).sin()).collect();
            let mut f = RealFft::new(n);
            let mut re = vec![0.0; n / 2 + 1];
            let mut im = vec![0.0; n / 2 + 1];
            f.forward(&x, &mut re, &mut im);
            for (k, (dr, di)) in naive_dft(&x).into_iter().enumerate() {
                let tol = 1e-3 * (n as f64).sqrt();
                assert!((re[k] as f64 - dr).abs() < tol, "n={n} k={k} re {} vs {dr}", re[k]);
                assert!((im[k] as f64 - di).abs() < tol, "n={n} k={k} im {} vs {di}", im[k]);
            }
        }
    }

    #[test]
    fn complex_fft_matches_dft() {
        let n = 256;
        let re0: Vec<f32> = (0..n).map(|i| ((i * 31 % 17) as f32 - 8.0) / 8.0).collect();
        let im0: Vec<f32> = (0..n).map(|i| ((i * 13 % 11) as f32 - 5.0) / 5.0).collect();
        let (mut re, mut im) = (re0.clone(), im0.clone());
        Fft::new(n).forward(&mut re, &mut im);
        for k in [0usize, 1, 7, 100, 255] {
            let (mut sr, mut si) = (0.0f64, 0.0f64);
            for t in 0..n {
                let a = -2.0 * PI * (k * t) as f64 / n as f64;
                let (c, s) = (a.cos(), a.sin());
                sr += re0[t] as f64 * c - im0[t] as f64 * s;
                si += re0[t] as f64 * s + im0[t] as f64 * c;
            }
            assert!((re[k] as f64 - sr).abs() < 1e-3 && (im[k] as f64 - si).abs() < 1e-3, "k {k}");
        }
    }

    #[test]
    fn sine_lands_in_bin() {
        let n = 1024;
        let x: Vec<f32> = (0..n).map(|i| (2.0 * PI * 64.0 * i as f64 / n as f64).sin() as f32).collect();
        let mut f = RealFft::new(n);
        let mut p = vec![0.0; n / 2 + 1];
        let (mut sr, mut si) = (Vec::new(), Vec::new());
        f.power(&x, &mut p, &mut sr, &mut si);
        let peak = p.iter().enumerate().max_by(|a, b| a.1.partial_cmp(b.1).unwrap()).unwrap().0;
        assert_eq!(peak, 64);
        assert!((p[64].sqrt() - (n as f32 / 2.0)).abs() < 1.0);
    }
}
