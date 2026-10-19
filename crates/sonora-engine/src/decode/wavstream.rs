//! Sequential (non-seekable) WAVE header parsing, used to read FFmpeg's
//! `-f wav` output from a pipe. The stream's `data` size is usually a
//! placeholder because FFmpeg cannot seek back to patch it, so callers read
//! samples until end of stream.

use super::pcm::PcmEncoding;
use std::io::{self, Read};

/// Format of a streamed WAVE file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamFormat {
    pub channels: u16,
    pub sample_rate: u32,
    pub encoding: PcmEncoding,
    pub block_align: usize,
}

const MAX_SKIPPED_HEADER_BYTES: u64 = 16 << 20;

fn read_exact_or_eof(r: &mut impl Read, buf: &mut [u8]) -> io::Result<bool> {
    let mut got = 0;
    while got < buf.len() {
        match r.read(&mut buf[got..]) {
            Ok(0) => return Ok(false),
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(true)
}

fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

/// Reads RIFF/RF64 chunk headers up to the start of the `data` chunk and
/// returns the stream format. Returns `Ok(None)` if the stream ended before
/// any header byte arrived (the producer failed before writing output).
pub fn read_header(r: &mut impl Read) -> io::Result<Option<StreamFormat>> {
    let mut riff = [0u8; 12];
    let mut first = [0u8; 1];
    if !read_exact_or_eof(r, &mut first)? {
        return Ok(None);
    }
    riff[0] = first[0];
    if !read_exact_or_eof(r, &mut riff[1..])? {
        return Err(bad("stream ended inside the RIFF header"));
    }
    let tag = &riff[0..4];
    if !(tag == b"RIFF" || tag == b"RF64" || tag == b"BW64") || &riff[8..12] != b"WAVE" {
        return Err(bad("decoder output is not a WAVE stream"));
    }
    let mut format: Option<(u16, u16, u32, u16, u16)> = None; // tag, channels, rate, align, bits
    let mut skipped = 0u64;
    loop {
        let mut hdr = [0u8; 8];
        if !read_exact_or_eof(r, &mut hdr)? {
            return Err(bad("stream ended before the data chunk"));
        }
        let id = [hdr[0], hdr[1], hdr[2], hdr[3]];
        let size = u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]) as u64;
        match &id {
            b"fmt " => {
                if !(16..=1024).contains(&size) {
                    return Err(bad("invalid fmt chunk size"));
                }
                let mut b = vec![0u8; size as usize + (size as usize & 1)];
                if !read_exact_or_eof(r, &mut b)? {
                    return Err(bad("stream ended inside the fmt chunk"));
                }
                let mut tag = u16::from_le_bytes([b[0], b[1]]);
                let channels = u16::from_le_bytes([b[2], b[3]]);
                let rate = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
                let align = u16::from_le_bytes([b[12], b[13]]);
                let bits = u16::from_le_bytes([b[14], b[15]]);
                if tag == 0xFFFE && b.len() >= 26 {
                    tag = u16::from_le_bytes([b[24], b[25]]);
                }
                format = Some((tag, channels, rate, align, bits));
            }
            b"data" => {
                let (tag, channels, rate, align, bits) = format.ok_or_else(|| bad("data chunk before fmt chunk"))?;
                if channels == 0 || channels > 64 {
                    return Err(bad("invalid channel count"));
                }
                if rate == 0 || rate > 1_536_000 {
                    return Err(bad("invalid sample rate"));
                }
                let bytes = (align as usize) / channels as usize;
                let encoding = match (tag, bytes, bits) {
                    (1, 2, _) => PcmEncoding::S16Le,
                    (1, 3, _) => PcmEncoding::S24Le,
                    (1, 4, _) => PcmEncoding::S32Le,
                    (3, 4, _) => PcmEncoding::F32Le,
                    (3, 8, _) => PcmEncoding::F64Le,
                    _ => return Err(bad("unexpected PCM layout from decoder")),
                };
                return Ok(Some(StreamFormat { channels, sample_rate: rate, encoding, block_align: align as usize }));
            }
            _ => {
                // LIST/fact/ds64 or anything else: skip (with a sanity bound).
                let skip = size + (size & 1);
                skipped += skip;
                if skipped > MAX_SKIPPED_HEADER_BYTES {
                    return Err(bad("implausibly large WAVE header"));
                }
                let copied = io::copy(&mut r.take(skip), &mut io::sink())?;
                if copied < skip {
                    return Err(bad("stream ended inside a header chunk"));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(tag: u16, extensible: bool, channels: u16, rate: u32, bits: u16, extra_chunk: bool) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        v.extend_from_slice(b"WAVE");
        let align = channels * bits / 8;
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&(if extensible { 0xFFFEu16 } else { tag }).to_le_bytes());
        fmt.extend_from_slice(&channels.to_le_bytes());
        fmt.extend_from_slice(&rate.to_le_bytes());
        fmt.extend_from_slice(&(rate * align as u32).to_le_bytes());
        fmt.extend_from_slice(&align.to_le_bytes());
        fmt.extend_from_slice(&bits.to_le_bytes());
        if extensible {
            fmt.extend_from_slice(&22u16.to_le_bytes());
            fmt.extend_from_slice(&bits.to_le_bytes());
            fmt.extend_from_slice(&3u32.to_le_bytes());
            fmt.extend_from_slice(&tag.to_le_bytes());
            fmt.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71]);
        }
        v.extend_from_slice(b"fmt ");
        v.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
        v.extend_from_slice(&fmt);
        if extra_chunk {
            v.extend_from_slice(b"LIST");
            v.extend_from_slice(&5u32.to_le_bytes());
            v.extend_from_slice(b"INFOx\0");
        }
        v.extend_from_slice(b"data");
        v.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        v
    }

    #[test]
    fn parses_extensible_float_with_list_chunk() {
        let mut h = header(3, true, 2, 44_100, 32, true);
        h.extend_from_slice(&[0u8; 16]);
        let mut cur = std::io::Cursor::new(h);
        let f = read_header(&mut cur).unwrap().unwrap();
        assert_eq!(f.channels, 2);
        assert_eq!(f.sample_rate, 44_100);
        assert_eq!(f.encoding, PcmEncoding::F32Le);
        assert_eq!(f.block_align, 8);
    }

    #[test]
    fn parses_plain_s32() {
        let h = header(1, false, 1, 48_000, 32, false);
        let f = read_header(&mut std::io::Cursor::new(h)).unwrap().unwrap();
        assert_eq!(f.encoding, PcmEncoding::S32Le);
        assert_eq!(f.channels, 1);
    }

    #[test]
    fn empty_stream_is_none_and_garbage_is_error() {
        assert!(read_header(&mut std::io::Cursor::new(Vec::<u8>::new())).unwrap().is_none());
        assert!(read_header(&mut std::io::Cursor::new(b"not a wave stream at all".to_vec())).is_err());
    }
}
