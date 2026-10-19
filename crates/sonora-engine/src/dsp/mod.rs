//! Signal-processing building blocks.

pub mod biquad;
pub mod fft;
pub mod kweight;
pub mod truepeak;

/// Periodic Hann window of length `n` (suited to spectral averaging).
pub fn hann(n: usize) -> Vec<f32> {
    (0..n).map(|i| (0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos()) as f32).collect()
}
