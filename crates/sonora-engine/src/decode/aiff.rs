//! Native AIFF / AIFF-C decoder (uncompressed big- and little-endian PCM and
//! IEEE float). Other AIFF-C compression types fall back to FFmpeg.

use super::pcm::{convert, PcmEncoding};
use super::sniff::read_up_to;
use super::{ChecksumStatus, DecodeStats, PcmSource, SampleKind, StreamInfo};
use crate::error::AnalysisError;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

pub enum OpenError {
    NotNative(AiffHeader),
    Failed(AnalysisError),
}

impl From<std::io::Error> for OpenError {
    fn from(e: std::io::Error) -> Self {
        OpenError::Failed(AnalysisError::Io(e.to_string()))
    }
}

#[derive(Clone, Debug)]
pub struct AiffHeader {
    pub aifc: bool,
    pub channels: u16,
    pub frames: u32,
    pub sample_size: u16,
    pub sample_rate: f64,
    pub compression: [u8; 4],
    pub encoding: Option<PcmEncoding>,
    pub data_offset: u64,
    pub data_len: u64,
    pub truncated: bool,
    pub text_tags: Vec<(String, String)>,
    pub id3_chunk: Option<(u64, u64)>,
}

impl AiffHeader {
    pub fn compression_str(&self) -> String {
        String::from_utf8_lossy(&self.compression).trim().to_string()
    }
}

fn corrupt(msg: &str) -> OpenError {
    OpenError::Failed(AnalysisError::Corrupt(msg.to_string()))
}

fn u16be(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}
fn u32be(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

/// Decodes an IEEE 754 80-bit extended float (big-endian), as used for the AIFF sample rate.
pub fn extended_to_f64(b: &[u8]) -> f64 {
    let sign = if b[0] & 0x80 != 0 { -1.0 } else { 1.0 };
    let exp = (((b[0] & 0x7F) as i32) << 8) | b[1] as i32;
    let mant = u64::from_be_bytes([b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9]]);
    if exp == 0 && mant == 0 {
        return 0.0;
    }
    if exp == 0x7FFF {
        return f64::NAN;
    }
    sign * (mant as f64) * 2f64.powi(exp - 16383 - 63)
}

pub fn parse_header(path: &Path) -> Result<AiffHeader, OpenError> {
    let mut f = File::open(path)?;
    let file_len = f.metadata()?.len();
    parse_header_from(&mut f, file_len, false)
}

/// Parses the chunk structure from any reader of a stream `file_len` bytes
/// long. With `stop_at_audio`, chunks after the sound data are not looked
/// at (a file still arriving has not got them yet).
pub fn parse_header_from<R: Read + Seek>(f: &mut R, file_len: u64, stop_at_audio: bool) -> Result<AiffHeader, OpenError> {
    let mut hdr = [0u8; 12];
    if read_up_to(f, &mut hdr)? < 12 {
        return Err(corrupt("file too short for an AIFF header"));
    }
    if &hdr[0..4] != b"FORM" {
        return Err(corrupt("missing FORM signature"));
    }
    let aifc = match &hdr[8..12] {
        b"AIFF" => false,
        b"AIFC" => true,
        _ => return Err(corrupt("FORM is not AIFF/AIFC")),
    };
    let mut pos = 12u64;
    let mut comm: Option<Vec<u8>> = None;
    let mut ssnd: Option<(u64, u64)> = None;
    let mut text_tags = Vec::new();
    let mut id3_chunk = None;
    let mut ch = [0u8; 8];
    while pos + 8 <= file_len {
        f.seek(SeekFrom::Start(pos))?;
        if read_up_to(f, &mut ch)? < 8 {
            break;
        }
        let id = [ch[0], ch[1], ch[2], ch[3]];
        let size = u32be(&ch[4..8]) as u64;
        let body = pos + 8;
        match &id {
            b"COMM" => {
                if !(18..=1024).contains(&size) {
                    return Err(corrupt("invalid COMM chunk size"));
                }
                let mut b = vec![0u8; size as usize];
                if read_up_to(f, &mut b)? < size as usize {
                    return Err(corrupt("truncated COMM chunk"));
                }
                comm = Some(b);
            }
            b"SSND" => {
                let mut b = [0u8; 8];
                if read_up_to(f, &mut b)? < 8 {
                    return Err(corrupt("truncated SSND chunk"));
                }
                let offset = u32be(&b[0..4]) as u64;
                ssnd = Some((body + 8 + offset, size.saturating_sub(8 + offset)));
                if stop_at_audio && comm.is_some() {
                    break;
                }
            }
            b"NAME" | b"AUTH" | b"ANNO" | b"(c) " => {
                if size < 4096 {
                    let mut b = vec![0u8; size as usize];
                    let n = read_up_to(f, &mut b)?;
                    let text = String::from_utf8_lossy(&b[..n]).trim_end_matches('\0').trim().to_string();
                    let key = match &id {
                        b"NAME" => "title",
                        b"AUTH" => "artist",
                        b"ANNO" => "comment",
                        _ => "copyright",
                    };
                    if !text.is_empty() {
                        text_tags.push((key.to_string(), text));
                    }
                }
            }
            b"ID3 " | b"id3 " => id3_chunk = Some((body, size)),
            _ => {}
        }
        pos = body.saturating_add(size).saturating_add(size & 1);
    }
    let comm = comm.ok_or_else(|| corrupt("missing COMM chunk"))?;
    let (data_offset, declared) = ssnd.ok_or_else(|| corrupt("missing SSND chunk"))?;
    let channels = u16be(&comm[0..2]);
    let frames = u32be(&comm[2..6]);
    let sample_size = u16be(&comm[6..8]);
    let sample_rate = extended_to_f64(&comm[8..18]);
    let compression = if aifc && comm.len() >= 22 { [comm[18], comm[19], comm[20], comm[21]] } else { *b"NONE" };
    if channels == 0 || channels > 32 {
        return Err(corrupt("invalid channel count"));
    }
    if !sample_rate.is_finite() || !(1.0..=1_536_000.0).contains(&sample_rate) {
        return Err(corrupt("invalid sample rate"));
    }
    let bytes = (sample_size as usize).div_ceil(8);
    let encoding = match (&compression, bytes) {
        (b"NONE" | b"twos", 1) => Some(PcmEncoding::S8),
        (b"NONE" | b"twos", 2) => Some(PcmEncoding::S16Be),
        (b"NONE" | b"twos", 3) => Some(PcmEncoding::S24Be),
        (b"NONE" | b"twos", 4) => Some(PcmEncoding::S32Be),
        (b"sowt", 2) => Some(PcmEncoding::S16Le),
        (b"sowt", 3) => Some(PcmEncoding::S24Le),
        (b"sowt", 4) => Some(PcmEncoding::S32Le),
        (b"in24", _) => Some(PcmEncoding::S24Be),
        (b"in32", _) => Some(PcmEncoding::S32Be),
        (b"raw ", 1) => Some(PcmEncoding::U8),
        (b"fl32" | b"FL32", _) => Some(PcmEncoding::F32Be),
        (b"fl64" | b"FL64", _) => Some(PcmEncoding::F64Be),
        _ => None,
    };
    let available = file_len.saturating_sub(data_offset);
    let (data_len, truncated) = match encoding {
        Some(e) => {
            let expected = (frames as u64 * channels as u64 * e.bytes() as u64).min(declared);
            (expected.min(available), expected > available)
        }
        None => (declared.min(available), declared > available),
    };
    Ok(AiffHeader {
        aifc,
        channels,
        frames,
        sample_size,
        sample_rate,
        compression,
        encoding,
        data_offset,
        data_len,
        truncated,
        text_tags,
        id3_chunk,
    })
}

pub struct AiffSource {
    reader: BufReader<Box<dyn super::ReadSeek>>,
    info: StreamInfo,
    enc: PcmEncoding,
    block_align: usize,
    remaining: u64,
    raw: Vec<u8>,
    or_mask: u32,
    truncated: bool,
}

impl AiffSource {
    pub fn open(path: &Path) -> Result<Self, OpenError> {
        let f = File::open(path)?;
        let len = f.metadata()?.len();
        Self::open_reader(Box::new(f), len, false)
    }

    /// Decodes from any reader of a stream `len` bytes long (see [`parse_header_from`]).
    pub fn open_reader(mut f: Box<dyn super::ReadSeek>, len: u64, stop_at_audio: bool) -> Result<Self, OpenError> {
        let h = parse_header_from(&mut f, len, stop_at_audio)?;
        let enc = match h.encoding {
            Some(e) => e,
            None => return Err(OpenError::NotNative(h)),
        };
        let block_align = enc.bytes() * h.channels as usize;
        f.seek(SeekFrom::Start(h.data_offset))?;
        let kind = if enc.is_float() { SampleKind::Float } else { SampleKind::Int };
        let bits = if enc.is_float() { enc.container_bits() } else { h.sample_size.min(enc.container_bits()) };
        Ok(AiffSource {
            reader: BufReader::with_capacity(1 << 18, f),
            info: StreamInfo {
                sample_rate: h.sample_rate.round() as u32,
                channels: h.channels,
                kind,
                bits: Some(bits),
                frames_hint: Some(h.frames as u64),
                decoder: "native-aiff",
            },
            enc,
            block_align,
            remaining: h.data_len - (h.data_len % block_align as u64),
            raw: Vec::new(),
            or_mask: 0,
            truncated: h.truncated,
        })
    }
}

impl PcmSource for AiffSource {
    fn info(&self) -> &StreamInfo {
        &self.info
    }

    fn read(&mut self, out: &mut Vec<f32>, max_frames: usize) -> Result<usize, AnalysisError> {
        out.clear();
        if self.remaining == 0 {
            return Ok(0);
        }
        let want = ((max_frames * self.block_align) as u64).min(self.remaining) as usize;
        self.raw.resize(want, 0);
        let got = read_up_to(&mut self.reader, &mut self.raw)?;
        let whole = got - (got % self.block_align);
        if whole < want {
            self.truncated = true;
            self.remaining = 0;
        } else {
            self.remaining -= whole as u64;
        }
        convert(&self.raw[..whole], self.enc, out, &mut self.or_mask);
        Ok(whole / self.block_align)
    }

    fn finish(&mut self) -> DecodeStats {
        DecodeStats {
            int_or_mask: if self.info.kind == SampleKind::Int { Some(self.or_mask) } else { None },
            decode_errors: 0,
            truncated: self.truncated,
            messages: Vec::new(),
            checksum: ChecksumStatus::NotApplicable,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extended_float() {
        // 44100 Hz encoded as 80-bit extended.
        let b = [0x40, 0x0E, 0xAC, 0x44, 0, 0, 0, 0, 0, 0];
        assert_eq!(extended_to_f64(&b), 44100.0);
        let b48 = [0x40, 0x0E, 0xBB, 0x80, 0, 0, 0, 0, 0, 0];
        assert_eq!(extended_to_f64(&b48), 48000.0);
    }

    fn write_aiff(path: &Path, rate_ext: [u8; 10], channels: u16, samples: &[i16]) {
        let frames = samples.len() as u32 / channels as u32;
        let data_len = samples.len() * 2;
        let mut b = Vec::new();
        b.extend_from_slice(b"FORM");
        b.extend_from_slice(&((4 + 26 + 16 + data_len) as u32).to_be_bytes());
        b.extend_from_slice(b"AIFF");
        b.extend_from_slice(b"COMM");
        b.extend_from_slice(&18u32.to_be_bytes());
        b.extend_from_slice(&channels.to_be_bytes());
        b.extend_from_slice(&frames.to_be_bytes());
        b.extend_from_slice(&16u16.to_be_bytes());
        b.extend_from_slice(&rate_ext);
        b.extend_from_slice(b"SSND");
        b.extend_from_slice(&((8 + data_len) as u32).to_be_bytes());
        b.extend_from_slice(&0u32.to_be_bytes());
        b.extend_from_slice(&0u32.to_be_bytes());
        for s in samples {
            b.extend_from_slice(&s.to_be_bytes());
        }
        std::fs::write(path, b).unwrap();
    }

    #[test]
    fn decodes_pcm16() {
        let dir = std::env::temp_dir().join(format!("sonora-aiff-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.aiff");
        let samples: Vec<i16> = (0..1000).map(|i| (i * 30 - 15000) as i16).collect();
        write_aiff(&p, [0x40, 0x0E, 0xAC, 0x44, 0, 0, 0, 0, 0, 0], 2, &samples);
        let mut src = AiffSource::open(&p).ok().expect("open");
        assert_eq!(src.info().sample_rate, 44100);
        assert_eq!(src.info().channels, 2);
        let mut buf = Vec::new();
        let n = src.read(&mut buf, 10_000).unwrap();
        assert_eq!(n, 500);
        assert_eq!(buf[0], -15000.0 / 32768.0);
        assert_eq!(src.read(&mut buf, 10).unwrap(), 0);
    }
}
