//! Native RIFF/RF64/BW64 WAVE decoder for PCM integer and IEEE float data.
//! Compressed WAVE payloads (MP3-in-WAV, ADPCM, A-law...) are reported via
//! [`OpenError::NotNative`] so the caller can fall back to FFmpeg.

use super::pcm::{convert, PcmEncoding};
use super::sniff::read_up_to;
use super::{ChecksumStatus, DecodeStats, PcmSource, SampleKind, StreamInfo};
use crate::error::AnalysisError;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

pub enum OpenError {
    /// The file is a valid WAVE but uses a codec this decoder does not handle.
    NotNative(WavHeader),
    Failed(AnalysisError),
}

impl From<std::io::Error> for OpenError {
    fn from(e: std::io::Error) -> Self {
        OpenError::Failed(AnalysisError::Io(e.to_string()))
    }
}

/// Parsed WAVE header.
#[derive(Clone, Debug)]
pub struct WavHeader {
    pub format_tag: u16,
    pub channels: u16,
    pub sample_rate: u32,
    pub byte_rate: u32,
    pub block_align: u16,
    pub bits_per_sample: u16,
    pub valid_bits: Option<u16>,
    pub encoding: Option<PcmEncoding>,
    pub data_offset: u64,
    /// Bytes of audio actually available in the file.
    pub data_len: u64,
    /// Declared data size exceeds what the file contains.
    pub truncated: bool,
    pub rf64: bool,
    /// RIFF INFO tags (title, artist, ...) if present.
    pub info_tags: Vec<(String, String)>,
    /// Offset/length of an embedded `id3 ` chunk, if present.
    pub id3_chunk: Option<(u64, u64)>,
}

impl WavHeader {
    pub fn codec_name(&self) -> &'static str {
        match self.format_tag {
            0x0001 => "pcm",
            0x0003 => "pcm_float",
            0x0006 => "alaw",
            0x0007 => "mulaw",
            0x0002 => "adpcm_ms",
            0x0011 => "adpcm_ima",
            0x0050 => "mp2",
            0x0055 => "mp3",
            0x00FF | 0x1610 => "aac",
            0xFFFE => "extensible",
            _ => "unknown",
        }
    }

    pub fn frames(&self) -> Option<u64> {
        if self.block_align == 0 {
            None
        } else {
            Some(self.data_len / self.block_align as u64)
        }
    }
}

fn u16le(b: &[u8]) -> u16 {
    u16::from_le_bytes([b[0], b[1]])
}
fn u32le(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}
fn u64le(b: &[u8]) -> u64 {
    u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
}

fn corrupt(msg: &str) -> OpenError {
    OpenError::Failed(AnalysisError::Corrupt(msg.to_string()))
}

/// Parses the WAVE chunk structure without reading audio.
pub fn parse_header(path: &Path) -> Result<WavHeader, OpenError> {
    let mut f = File::open(path)?;
    let file_len = f.metadata()?.len();
    parse_header_from(&mut f, file_len, false)
}

/// Parses the chunk structure from any reader of a stream `file_len` bytes
/// long. With `stop_at_audio`, chunks after the audio are not looked at (a
/// file still arriving has not got them yet).
pub fn parse_header_from<R: Read + Seek>(f: &mut R, file_len: u64, stop_at_audio: bool) -> Result<WavHeader, OpenError> {
    let mut hdr = [0u8; 12];
    if read_up_to(f, &mut hdr)? < 12 {
        return Err(corrupt("file too short for a WAVE header"));
    }
    let rf64 = &hdr[0..4] == b"RF64" || &hdr[0..4] == b"BW64";
    if !(&hdr[0..4] == b"RIFF" || rf64) || &hdr[8..12] != b"WAVE" {
        return Err(corrupt("missing RIFF/WAVE signature"));
    }
    let mut pos = 12u64;
    let mut ds64_data: Option<u64> = None;
    let mut fmt: Option<Vec<u8>> = None;
    let mut data: Option<(u64, u64, bool)> = None; // (offset, declared, placeholder)
    let mut info_tags = Vec::new();
    let mut id3_chunk = None;
    let mut chunk_hdr = [0u8; 8];
    while pos + 8 <= file_len {
        f.seek(SeekFrom::Start(pos))?;
        if read_up_to(f, &mut chunk_hdr)? < 8 {
            break;
        }
        let id = [chunk_hdr[0], chunk_hdr[1], chunk_hdr[2], chunk_hdr[3]];
        let size32 = u32le(&chunk_hdr[4..8]);
        let body = pos + 8;
        match &id {
            b"ds64" => {
                let mut b = vec![0u8; (size32 as usize).min(64)];
                read_up_to(f, &mut b)?;
                if b.len() >= 16 {
                    ds64_data = Some(u64le(&b[8..16]));
                }
            }
            b"fmt " => {
                if !(16..=1024).contains(&size32) {
                    return Err(corrupt("invalid fmt chunk size"));
                }
                let mut b = vec![0u8; size32 as usize];
                if read_up_to(f, &mut b)? < size32 as usize {
                    return Err(corrupt("truncated fmt chunk"));
                }
                fmt = Some(b);
            }
            b"data" => {
                let placeholder = size32 == 0xFFFF_FFFF || (size32 == 0 && !rf64);
                let declared = if rf64 && size32 == 0xFFFF_FFFF {
                    ds64_data.unwrap_or(file_len.saturating_sub(body))
                } else {
                    size32 as u64
                };
                data = Some((body, declared, placeholder));
                // Metadata chunks may follow the data; keep scanning when sizes are sane.
                if placeholder || (stop_at_audio && fmt.is_some()) {
                    break;
                }
            }
            b"LIST" => {
                if (4..1 << 20).contains(&size32) {
                    let mut b = vec![0u8; size32 as usize];
                    let n = read_up_to(f, &mut b)?;
                    b.truncate(n);
                    if b.len() >= 4 && &b[0..4] == b"INFO" {
                        info_tags = parse_info_list(&b[4..]);
                    }
                }
            }
            b"id3 " | b"ID3 " => {
                id3_chunk = Some((body, size32 as u64));
            }
            _ => {}
        }
        let size = if &id == b"data" { data.map(|d| d.1).unwrap_or(size32 as u64) } else { size32 as u64 };
        pos = body.saturating_add(size).saturating_add(size & 1);
    }
    let fmt = fmt.ok_or_else(|| corrupt("missing fmt chunk"))?;
    let (data_offset, declared, placeholder) = data.ok_or_else(|| corrupt("missing data chunk"))?;
    let available = file_len.saturating_sub(data_offset);
    let truncated = !placeholder && declared > available;
    let data_len = if placeholder { available } else { declared.min(available) };

    let mut format_tag = u16le(&fmt[0..2]);
    let channels = u16le(&fmt[2..4]);
    let sample_rate = u32le(&fmt[4..8]);
    let byte_rate = u32le(&fmt[8..12]);
    let block_align = u16le(&fmt[12..14]);
    let bits_per_sample = u16le(&fmt[14..16]);
    let mut valid_bits = None;
    if format_tag == 0xFFFE && fmt.len() >= 40 {
        let vb = u16le(&fmt[18..20]);
        if vb > 0 && vb <= bits_per_sample {
            valid_bits = Some(vb);
        }
        // Sub-format GUID: first two bytes carry the format code.
        format_tag = u16le(&fmt[24..26]);
    }
    if channels == 0 || channels > 32 {
        return Err(corrupt("invalid channel count"));
    }
    if sample_rate == 0 || sample_rate > 1_536_000 {
        return Err(corrupt("invalid sample rate"));
    }
    let container_bytes = if channels > 0 { block_align as usize / channels as usize } else { 0 };
    let encoding = match (format_tag, container_bytes) {
        (1, 1) => Some(PcmEncoding::U8),
        (1, 2) => Some(PcmEncoding::S16Le),
        (1, 3) => Some(PcmEncoding::S24Le),
        (1, 4) => Some(PcmEncoding::S32Le),
        (3, 4) => Some(PcmEncoding::F32Le),
        (3, 8) => Some(PcmEncoding::F64Le),
        _ => None,
    };
    if encoding.is_some() && block_align as usize != container_bytes * channels as usize {
        return Err(corrupt("block alignment does not match channel layout"));
    }
    Ok(WavHeader {
        format_tag,
        channels,
        sample_rate,
        byte_rate,
        block_align,
        bits_per_sample,
        valid_bits,
        encoding,
        data_offset,
        data_len,
        truncated,
        rf64,
        info_tags,
        id3_chunk,
    })
}

fn parse_info_list(b: &[u8]) -> Vec<(String, String)> {
    let mut tags = Vec::new();
    let mut i = 0;
    while i + 8 <= b.len() {
        let id = String::from_utf8_lossy(&b[i..i + 4]).to_string();
        let size = u32le(&b[i + 4..i + 8]) as usize;
        let start = i + 8;
        let end = (start + size).min(b.len());
        let raw = &b[start..end];
        let text = String::from_utf8_lossy(raw).trim_end_matches('\0').trim().to_string();
        let key = match id.as_str() {
            "INAM" => Some("title"),
            "IART" => Some("artist"),
            "IPRD" => Some("album"),
            "IGNR" => Some("genre"),
            "ICRD" => Some("date"),
            "ICMT" => Some("comment"),
            "ISFT" => Some("encoder"),
            _ => None,
        };
        if let Some(k) = key {
            if !text.is_empty() {
                tags.push((k.to_string(), text));
            }
        }
        i = start + size + (size & 1);
    }
    tags
}

pub struct WavSource {
    reader: BufReader<Box<dyn super::ReadSeek>>,
    info: StreamInfo,
    enc: PcmEncoding,
    block_align: usize,
    remaining: u64,
    raw: Vec<u8>,
    or_mask: u32,
    truncated: bool,
}

impl WavSource {
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
        f.seek(SeekFrom::Start(h.data_offset))?;
        let kind = if enc.is_float() { SampleKind::Float } else { SampleKind::Int };
        let bits = if enc.is_float() { enc.container_bits() } else { h.valid_bits.unwrap_or(h.bits_per_sample.min(enc.container_bits())) };
        Ok(WavSource {
            reader: BufReader::with_capacity(1 << 18, f),
            info: StreamInfo {
                sample_rate: h.sample_rate,
                channels: h.channels,
                kind,
                bits: Some(bits),
                frames_hint: h.frames(),
                decoder: "native-wav",
            },
            enc,
            block_align: h.block_align as usize,
            remaining: h.data_len - (h.data_len % h.block_align as u64),
            raw: Vec::new(),
            or_mask: 0,
            truncated: h.truncated,
        })
    }
}

impl PcmSource for WavSource {
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
            // File ended before the declared data size.
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

/// Writes a canonical PCM WAVE file (used by tests and fixture tooling).
pub fn write_wav(path: &Path, sample_rate: u32, channels: u16, bits: u16, float: bool, samples: &[f32]) -> std::io::Result<()> {
    use std::io::Write;
    let bytes_per = (bits / 8) as usize;
    let data_len = samples.len() * bytes_per;
    let mut buf = Vec::with_capacity(44 + data_len);
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
    buf.extend_from_slice(b"WAVE");
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes());
    buf.extend_from_slice(&(if float { 3u16 } else { 1u16 }).to_le_bytes());
    buf.extend_from_slice(&channels.to_le_bytes());
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    buf.extend_from_slice(&(sample_rate * channels as u32 * bytes_per as u32).to_le_bytes());
    buf.extend_from_slice(&(channels * bytes_per as u16).to_le_bytes());
    buf.extend_from_slice(&bits.to_le_bytes());
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&(data_len as u32).to_le_bytes());
    for &s in samples {
        match (float, bits) {
            (true, 32) => buf.extend_from_slice(&s.to_le_bytes()),
            (false, 16) => {
                let v = (s.clamp(-1.0, 1.0) * 32768.0).round().clamp(-32768.0, 32767.0) as i16;
                buf.extend_from_slice(&v.to_le_bytes());
            }
            (false, 24) => {
                let v = (s.clamp(-1.0, 1.0) as f64 * 8_388_608.0).round().clamp(-8_388_608.0, 8_388_607.0) as i32;
                buf.extend_from_slice(&v.to_le_bytes()[0..3]);
            }
            (false, 32) => {
                let v = (s.clamp(-1.0, 1.0) as f64 * 2_147_483_648.0).round().clamp(-2_147_483_648.0, 2_147_483_647.0) as i32;
                buf.extend_from_slice(&v.to_le_bytes());
            }
            _ => return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "unsupported format")),
        }
    }
    let mut f = File::create(path)?;
    f.write_all(&buf)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("sonora-wav-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn roundtrip_16_24_float() {
        let samples: Vec<f32> = (0..2000).map(|i| ((i as f32) * 0.01).sin() * 0.8).collect();
        for (bits, float) in [(16u16, false), (24, false), (32, true)] {
            let p = tmp(&format!("rt{bits}{float}.wav"));
            write_wav(&p, 44_100, 2, bits, float, &samples).unwrap();
            let mut src = WavSource::open(&p).ok().expect("open");
            assert_eq!(src.info().channels, 2);
            assert_eq!(src.info().sample_rate, 44_100);
            let mut all = Vec::new();
            let mut buf = Vec::new();
            loop {
                let n = src.read(&mut buf, 333).unwrap();
                if n == 0 {
                    break;
                }
                all.extend_from_slice(&buf);
            }
            assert_eq!(all.len(), samples.len());
            let tol = if float { 0.0 } else { 2.0 / (1u32 << (bits - 1)) as f32 };
            for (a, b) in all.iter().zip(&samples) {
                assert!((a - b).abs() <= tol + 1e-7, "bits {bits}: {a} vs {b}");
            }
            let stats = src.finish();
            assert!(!stats.truncated);
            assert_eq!(stats.int_or_mask.is_some(), !float);
        }
    }

    #[test]
    fn detects_truncation() {
        let samples = vec![0.25f32; 4000];
        let p = tmp("trunc.wav");
        write_wav(&p, 48_000, 2, 16, false, &samples).unwrap();
        let bytes = std::fs::read(&p).unwrap();
        std::fs::write(&p, &bytes[..bytes.len() - 1000]).unwrap();
        let h = parse_header(&p).ok().unwrap();
        assert!(h.truncated);
        let mut src = WavSource::open(&p).ok().unwrap();
        let mut buf = Vec::new();
        let mut frames = 0;
        loop {
            let n = src.read(&mut buf, 4096).unwrap();
            if n == 0 {
                break;
            }
            frames += n;
        }
        // 4000 samples = 2000 stereo frames = 8000 bytes; 1000 bytes removed.
        assert_eq!(frames, (8000 - 1000) / 4);
        assert!(src.finish().truncated);
    }

    #[test]
    fn rejects_garbage() {
        let p = tmp("garbage.wav");
        std::fs::write(&p, b"RIFF\x10\0\0\0WAVEjunkjunk").unwrap();
        assert!(matches!(parse_header(&p), Err(OpenError::Failed(_))));
    }
}
