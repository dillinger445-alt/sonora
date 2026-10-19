//! In-memory PCM source, used for synthetic test signals and by callers
//! that already hold decoded audio.

use super::{DecodeStats, PcmSource, SampleKind, StreamInfo};
use crate::error::AnalysisError;

pub struct MemorySource {
    info: StreamInfo,
    samples: Vec<f32>,
    pos: usize,
    int_or_mask: Option<u32>,
}

impl MemorySource {
    /// `samples` are interleaved frames.
    pub fn new(sample_rate: u32, channels: u16, kind: SampleKind, samples: Vec<f32>) -> Self {
        let frames = samples.len() / channels.max(1) as usize;
        MemorySource {
            info: StreamInfo {
                sample_rate,
                channels,
                kind,
                bits: if kind == SampleKind::Int { Some(24) } else { None },
                frames_hint: Some(frames as u64),
                decoder: "memory",
            },
            samples,
            pos: 0,
            int_or_mask: None,
        }
    }

    /// Simulates integer PCM at `bits`, quantising the samples and recording
    /// the raw OR mask exactly as a file decoder would.
    pub fn quantized(sample_rate: u32, channels: u16, bits: u16, samples: &[f32]) -> Self {
        let scale = (1u64 << (bits - 1)) as f64;
        let mut mask = 0u32;
        let q: Vec<f32> = samples
            .iter()
            .map(|&s| {
                let v = (s as f64 * scale).round().clamp(-scale, scale - 1.0) as i64;
                mask |= ((v as i32) << (32 - bits)) as u32;
                (v as f64 / scale) as f32
            })
            .collect();
        let mut src = MemorySource::new(sample_rate, channels, SampleKind::Int, q);
        src.info.bits = Some(bits);
        src.int_or_mask = Some(mask);
        src
    }

    pub fn set_bits(&mut self, bits: Option<u16>) {
        self.info.bits = bits;
    }
}

impl PcmSource for MemorySource {
    fn info(&self) -> &StreamInfo {
        &self.info
    }

    fn read(&mut self, out: &mut Vec<f32>, max_frames: usize) -> Result<usize, AnalysisError> {
        out.clear();
        let ch = self.info.channels.max(1) as usize;
        let remaining = (self.samples.len() - self.pos) / ch;
        let n = remaining.min(max_frames);
        out.extend_from_slice(&self.samples[self.pos..self.pos + n * ch]);
        self.pos += n * ch;
        Ok(n)
    }

    fn finish(&mut self) -> DecodeStats {
        DecodeStats { int_or_mask: self.int_or_mask, ..Default::default() }
    }
}
