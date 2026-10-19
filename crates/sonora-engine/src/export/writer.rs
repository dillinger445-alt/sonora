//! Streaming AIFF and WAV writers (16- or 24-bit PCM) with TPDF dither.
//!
//! Headers are written first with placeholder sizes and patched when the
//! writer is finished, so any length can be streamed without holding the
//! audio in memory. Tags go in an `ID3 ` chunk after the audio (AIFF, as
//! iTunes, Rekordbox and Serato write them) or in `LIST`/`INFO` and `id3 `
//! chunks before it (WAV).

use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PcmContainer {
    Aiff,
    Wav,
}

/// Largest audio payload a 32-bit RIFF/AIFF size field can describe (with headroom for tags).
const MAX_DATA_BYTES: u64 = u32::MAX as u64 - 16 * 1024 * 1024;

pub struct PcmWriter {
    out: BufWriter<File>,
    container: PcmContainer,
    channels: u16,
    bits: u16,
    frames: u64,
    dither: bool,
    rng: u64,
    buf: Vec<u8>,
    /// Offset of the RIFF/FORM size field, the data/SSND size field and (AIFF) the COMM frame count.
    outer_size_at: u64,
    data_size_at: u64,
    frames_at: Option<u64>,
    /// Tag written after the audio (AIFF).
    trailing_id3: Option<Vec<u8>>,
    clipped: u64,
}

/// Sample rate as an 80-bit IEEE 754 extended float (AIFF `COMM`).
fn ieee_extended(rate: u32) -> [u8; 10] {
    let mut out = [0u8; 10];
    if rate == 0 {
        return out;
    }
    let r = rate as f64;
    let exp = r.log2().floor() as i32;
    let mantissa = (r / 2f64.powi(exp) * 2f64.powi(63)) as u64;
    out[..2].copy_from_slice(&((exp + 16_383) as u16).to_be_bytes());
    out[2..].copy_from_slice(&mantissa.to_be_bytes());
    out
}

fn too_long() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, "the audio is too long for a WAV or AIFF file (over 4 GB)")
}

impl PcmWriter {
    /// Creates `path` and writes the headers. `id3` is a complete ID3v2 tag;
    /// `info` a RIFF `INFO` list body (WAV only).
    pub fn create(path: &Path, container: PcmContainer, sample_rate: u32, channels: u16, bits: u16, id3: Option<Vec<u8>>, info: Option<Vec<u8>>) -> std::io::Result<PcmWriter> {
        if bits != 16 && bits != 24 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "only 16- and 24-bit PCM is written"));
        }
        let file = File::create(path)?;
        let mut out = BufWriter::with_capacity(1 << 20, file);
        let block = channels as u32 * (bits as u32 / 8);
        let (outer_size_at, data_size_at, frames_at, trailing_id3);
        match container {
            PcmContainer::Wav => {
                out.write_all(b"RIFF\0\0\0\0WAVE")?;
                outer_size_at = 4;
                out.write_all(b"fmt ")?;
                out.write_all(&16u32.to_le_bytes())?;
                out.write_all(&1u16.to_le_bytes())?;
                out.write_all(&channels.to_le_bytes())?;
                out.write_all(&sample_rate.to_le_bytes())?;
                out.write_all(&(sample_rate * block).to_le_bytes())?;
                out.write_all(&(block as u16).to_le_bytes())?;
                out.write_all(&bits.to_le_bytes())?;
                let mut pos = 36u64;
                if let Some(info) = info.filter(|i| i.len() > 4) {
                    out.write_all(b"LIST")?;
                    out.write_all(&(info.len() as u32).to_le_bytes())?;
                    out.write_all(&info)?;
                    pos += 8 + info.len() as u64;
                    if info.len() % 2 == 1 {
                        out.write_all(&[0])?;
                        pos += 1;
                    }
                }
                if let Some(tag) = id3 {
                    out.write_all(b"id3 ")?;
                    out.write_all(&(tag.len() as u32).to_le_bytes())?;
                    out.write_all(&tag)?;
                    pos += 8 + tag.len() as u64;
                    if tag.len() % 2 == 1 {
                        out.write_all(&[0])?;
                        pos += 1;
                    }
                }
                out.write_all(b"data\0\0\0\0")?;
                data_size_at = pos + 4;
                frames_at = None;
                trailing_id3 = None;
            }
            PcmContainer::Aiff => {
                out.write_all(b"FORM\0\0\0\0AIFF")?;
                outer_size_at = 4;
                out.write_all(b"COMM")?;
                out.write_all(&18u32.to_be_bytes())?;
                out.write_all(&(channels as i16).to_be_bytes())?;
                out.write_all(&0u32.to_be_bytes())?;
                frames_at = Some(22);
                out.write_all(&(bits as i16).to_be_bytes())?;
                out.write_all(&ieee_extended(sample_rate))?;
                out.write_all(b"SSND\0\0\0\0")?;
                data_size_at = 42;
                out.write_all(&[0u8; 8])?; // offset, block size
                trailing_id3 = id3;
            }
        }
        Ok(PcmWriter {
            out,
            container,
            channels,
            bits,
            frames: 0,
            dither: false,
            rng: 0x2545_F491_4F6C_DD1D,
            buf: Vec::new(),
            outer_size_at,
            data_size_at,
            frames_at,
            trailing_id3,
            clipped: 0,
        })
    }

    /// Adds triangular (TPDF) dither of ±1 LSB before rounding: use it
    /// whenever the input carries more resolution than the output keeps.
    pub fn set_dither(&mut self, on: bool) {
        self.dither = on;
    }

    fn noise(&mut self) -> f64 {
        // xorshift64*: two uniforms make a triangular distribution.
        let mut next = || {
            self.rng ^= self.rng >> 12;
            self.rng ^= self.rng << 25;
            self.rng ^= self.rng >> 27;
            (self.rng.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
        };
        next() - next()
    }

    /// Writes interleaved samples (full scale ±1.0).
    pub fn write(&mut self, samples: &[f32]) -> std::io::Result<()> {
        let ch = self.channels as usize;
        let frames = samples.len() / ch.max(1);
        let bytes = (self.bits / 8) as usize;
        if (self.frames + frames as u64) * (ch * bytes) as u64 > MAX_DATA_BYTES {
            return Err(too_long());
        }
        let scale = if self.bits == 16 { 32_768.0 } else { 8_388_608.0 };
        self.buf.clear();
        self.buf.reserve(samples.len() * bytes);
        for &x in &samples[..frames * ch] {
            let mut v = x as f64 * scale;
            if self.dither {
                v += self.noise();
            }
            let r = v.round();
            let q = r.clamp(-scale, scale - 1.0);
            if q != r {
                self.clipped += 1;
            }
            let q = q as i32;
            match (self.container, self.bits) {
                (PcmContainer::Wav, 16) => self.buf.extend_from_slice(&(q as i16).to_le_bytes()),
                (PcmContainer::Aiff, 16) => self.buf.extend_from_slice(&(q as i16).to_be_bytes()),
                (PcmContainer::Wav, _) => self.buf.extend_from_slice(&q.to_le_bytes()[..3]),
                (PcmContainer::Aiff, _) => self.buf.extend_from_slice(&q.to_be_bytes()[1..]),
            }
        }
        self.out.write_all(&self.buf)?;
        self.frames += frames as u64;
        Ok(())
    }

    /// Samples that had to be clamped to full scale.
    pub fn clipped_samples(&self) -> u64 {
        self.clipped
    }

    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Writes trailing chunks, patches the sizes and returns the file size.
    pub fn finish(mut self) -> std::io::Result<u64> {
        let data_bytes = self.frames * self.channels as u64 * (self.bits / 8) as u64;
        let ssnd = match self.container {
            PcmContainer::Wav => data_bytes,
            PcmContainer::Aiff => data_bytes + 8,
        };
        if data_bytes % 2 == 1 {
            self.out.write_all(&[0])?;
        }
        if let Some(tag) = self.trailing_id3.take() {
            self.out.write_all(b"ID3 ")?;
            self.out.write_all(&(tag.len() as u32).to_be_bytes())?;
            self.out.write_all(&tag)?;
            if tag.len() % 2 == 1 {
                self.out.write_all(&[0])?;
            }
        }
        self.out.flush()?;
        let mut file = self.out.into_inner().map_err(|e| e.into_error())?;
        let total = file.seek(SeekFrom::End(0))?;
        let outer = (total - 8) as u32;
        let be = self.container == PcmContainer::Aiff;
        let put = |file: &mut File, at: u64, v: u32| -> std::io::Result<()> {
            file.seek(SeekFrom::Start(at))?;
            file.write_all(&if be { v.to_be_bytes() } else { v.to_le_bytes() })
        };
        put(&mut file, self.outer_size_at, outer)?;
        put(&mut file, self.data_size_at, ssnd as u32)?;
        if let Some(at) = self.frames_at {
            put(&mut file, at, self.frames as u32)?;
        }
        file.sync_all()?;
        Ok(total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::{aiff::AiffSource, wav::WavSource, PcmSource};

    fn read_all(src: &mut dyn PcmSource) -> Vec<f32> {
        let mut all = Vec::new();
        let mut buf = Vec::new();
        while src.read(&mut buf, 4096).unwrap() > 0 {
            all.extend_from_slice(&buf);
        }
        all
    }

    #[test]
    fn extended_float_rates() {
        assert_eq!(ieee_extended(44_100), [0x40, 0x0E, 0xAC, 0x44, 0, 0, 0, 0, 0, 0]);
        assert_eq!(ieee_extended(48_000), [0x40, 0x0E, 0xBB, 0x80, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn written_files_decode_back_exactly() {
        let dir = std::env::temp_dir().join(format!("sonora-writer-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // 16-bit values: written without dither they must come back bit for bit.
        let samples: Vec<f32> = (0..20_001).map(|i| ((i * 37 % 2000) as f32 - 1000.0) / 32_768.0).collect();
        let samples = &samples[..20_000];
        for (container, bits) in [(PcmContainer::Wav, 16), (PcmContainer::Aiff, 16), (PcmContainer::Wav, 24), (PcmContainer::Aiff, 24)] {
            let p = dir.join(format!("w{bits}.{}", if container == PcmContainer::Wav { "wav" } else { "aiff" }));
            let tag = crate::export::id3::build(&crate::export::id3::TagSet { title: Some("Round trip".into()), bpm: Some(128.0), ..Default::default() });
            let mut w = PcmWriter::create(&p, container, 44_100, 2, bits, Some(tag), Some(b"INFO".to_vec())).unwrap();
            for c in samples.chunks(3001 * 2) {
                w.write(c).unwrap();
            }
            assert_eq!(w.frames(), 10_000);
            w.finish().unwrap();
            let back = match container {
                PcmContainer::Wav => read_all(&mut WavSource::open(&p).ok().unwrap()),
                PcmContainer::Aiff => read_all(&mut AiffSource::open(&p).ok().unwrap()),
            };
            assert_eq!(back.len(), samples.len(), "{p:?}");
            assert!(back.iter().zip(samples).all(|(a, b)| (a - b).abs() < 1e-9), "{p:?}");
            let facts = crate::meta::probe_file(&p, &crate::decode::ToolPaths::none()).unwrap();
            assert_eq!(facts.tags.title.as_deref(), Some("Round trip"), "{p:?}");
            assert_eq!(facts.tags.bpm, Some(128.0));
            assert_eq!(facts.bit_depth, Some(bits));
            assert!((facts.header_duration_secs.unwrap() - 10_000.0 / 44_100.0).abs() < 1e-6);
        }
    }

    #[test]
    fn dither_is_triangular_and_small() {
        let dir = std::env::temp_dir().join(format!("sonora-dither-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("d.wav");
        let mut w = PcmWriter::create(&p, PcmContainer::Wav, 44_100, 1, 16, None, None).unwrap();
        w.set_dither(true);
        w.write(&vec![0.25f32 / 32_768.0; 50_000]).unwrap();
        w.finish().unwrap();
        let back = read_all(&mut WavSource::open(&p).ok().unwrap());
        let ints: Vec<i32> = back.iter().map(|v| (v * 32_768.0).round() as i32).collect();
        assert!(ints.iter().all(|&v| (-1..=2).contains(&v)), "at most ±1 LSB of noise");
        let mean = ints.iter().sum::<i32>() as f64 / ints.len() as f64;
        assert!((mean - 0.25).abs() < 0.02, "dither keeps the sub-LSB level on average: {mean}");
    }
}
