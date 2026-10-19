//! Native FLAC decoder (RFC 9639).
//!
//! The decoder streams frame by frame with bounded memory: a byte window that
//! always holds at least one complete frame, plus one decoded block of samples
//! per channel. Every frame header is checked with its CRC-8 and every frame
//! with its CRC-16; the MD5 of the decoded audio is compared with the one the
//! encoder stored in STREAMINFO.
//!
//! Damaged input never panics. A frame whose CRC-16 fails, or whose body cannot
//! be parsed, is replaced with digital silence of the same length (the
//! behaviour of the reference decoder, which keeps the timeline intact); when
//! the stream loses sync the decoder scans forward for the next valid frame
//! header. Every such event is counted in [`DecodeStats::decode_errors`].

use super::sniff::{id3v2_size, read_up_to};
use super::{ChecksumStatus, DecodeStats, PcmSource, SampleKind, StreamInfo};
use crate::error::AnalysisError;
use crate::md5::Md5;
use crate::meta::vorbis_comment;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

pub enum OpenError {
    /// The stream is valid FLAC but uses a feature this decoder does not handle.
    NotNative(String),
    Failed(AnalysisError),
}

/// Facts from the FLAC metadata blocks.
#[derive(Clone, Debug, Default)]
pub struct FlacHeader {
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
    /// Total samples per channel declared in STREAMINFO (`None` when 0/unknown).
    pub total_frames: Option<u64>,
    /// MD5 of the unencoded audio (`None` when the encoder stored all zeros).
    pub md5: Option<[u8; 16]>,
    pub min_block_size: u16,
    pub max_block_size: u16,
    pub min_frame_size: u32,
    pub max_frame_size: u32,
    /// VORBIS_COMMENT vendor string (usually the encoder, e.g. "reference libFLAC 1.4.3").
    pub vendor: Option<String>,
    /// VORBIS_COMMENT tags with lower-case keys.
    pub tags: Vec<(String, String)>,
    pub has_picture: bool,
    /// Byte offset of the first audio frame.
    pub audio_offset: u64,
    /// The file starts with a (non-standard) ID3v2 tag before "fLaC".
    pub id3_prefix: bool,
}

/// Maximum number of diagnostic messages kept per file.
const MAX_MESSAGES: usize = 8;
/// Zero bytes kept after the valid data so the bit reader can always load 8 bytes.
const PAD: usize = 16;
/// Size of each read from the file.
const READ_CHUNK: usize = 256 * 1024;
/// Frames larger than this are treated as corrupt (a valid frame is < 2.2 MiB).
const MAX_FRAME_BYTES: usize = 32 << 20;

// ---------------------------------------------------------------------------
// CRCs

const fn crc8_table() -> [u8; 256] {
    let mut t = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u8;
        let mut j = 0;
        while j < 8 {
            c = if c & 0x80 != 0 { (c << 1) ^ 0x07 } else { c << 1 };
            j += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

const fn crc16_table() -> [u16; 256] {
    let mut t = [0u16; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = (i as u16) << 8;
        let mut j = 0;
        while j < 8 {
            c = if c & 0x8000 != 0 { (c << 1) ^ 0x8005 } else { c << 1 };
            j += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

static CRC8_TABLE: [u8; 256] = crc8_table();
static CRC16_TABLE: [u16; 256] = crc16_table();

/// CRC-8, polynomial x^8 + x^2 + x + 1, initial value 0 (frame headers).
fn crc8(data: &[u8]) -> u8 {
    data.iter().fold(0u8, |c, &b| CRC8_TABLE[(c ^ b) as usize])
}

/// CRC-16, polynomial x^16 + x^15 + x^2 + 1, initial value 0 (whole frames).
fn crc16(data: &[u8]) -> u16 {
    data.iter().fold(0u16, |c, &b| (c << 8) ^ CRC16_TABLE[((c >> 8) as u8 ^ b) as usize])
}

// ---------------------------------------------------------------------------
// Metadata

fn corrupt(msg: &str) -> AnalysisError {
    AnalysisError::Corrupt(msg.to_string())
}

fn parse_streaminfo(b: &[u8; 34], h: &mut FlacHeader) -> Result<(), AnalysisError> {
    h.min_block_size = u16::from_be_bytes([b[0], b[1]]);
    h.max_block_size = u16::from_be_bytes([b[2], b[3]]);
    h.min_frame_size = u32::from_be_bytes([0, b[4], b[5], b[6]]);
    h.max_frame_size = u32::from_be_bytes([0, b[7], b[8], b[9]]);
    let sample_rate = ((b[10] as u32) << 12) | ((b[11] as u32) << 4) | ((b[12] as u32) >> 4);
    let channels = ((b[12] >> 1) & 0x07) as u16 + 1;
    let bits = ((((b[12] & 1) << 4) | (b[13] >> 4)) as u16) + 1;
    let total = (((b[13] & 0x0F) as u64) << 32) | u32::from_be_bytes([b[14], b[15], b[16], b[17]]) as u64;
    if sample_rate == 0 {
        return Err(corrupt("STREAMINFO declares a sample rate of 0"));
    }
    h.sample_rate = sample_rate;
    h.channels = channels;
    h.bits_per_sample = bits;
    h.total_frames = if total > 0 { Some(total) } else { None };
    let mut md5 = [0u8; 16];
    md5.copy_from_slice(&b[18..34]);
    h.md5 = if md5.iter().all(|&x| x == 0) { None } else { Some(md5) };
    Ok(())
}

fn read_metadata(f: &mut File, file_len: u64) -> Result<FlacHeader, AnalysisError> {
    let mut h = FlacHeader::default();
    let mut head = [0u8; 10];
    let n = read_up_to(f, &mut head)?;
    let mut pos = 0u64;
    if let Some(size) = id3v2_size(&head[..n]) {
        h.id3_prefix = true;
        pos = size;
    }
    f.seek(SeekFrom::Start(pos))?;
    let mut magic = [0u8; 4];
    if read_up_to(f, &mut magic)? < 4 || &magic != b"fLaC" {
        return Err(corrupt("not a FLAC stream (missing \"fLaC\" signature)"));
    }
    pos += 4;
    let mut first = true;
    loop {
        let mut bh = [0u8; 4];
        if read_up_to(f, &mut bh)? < 4 {
            return Err(corrupt("the metadata section ends unexpectedly"));
        }
        let last = bh[0] & 0x80 != 0;
        let kind = bh[0] & 0x7F;
        let len = u32::from_be_bytes([0, bh[1], bh[2], bh[3]]) as u64;
        let body = pos + 4;
        if body + len > file_len {
            return Err(corrupt("a metadata block extends past the end of the file"));
        }
        if first && kind != 0 {
            return Err(corrupt("the first metadata block is not STREAMINFO"));
        }
        match kind {
            0 if first => {
                if len < 34 {
                    return Err(corrupt("the STREAMINFO block is too short"));
                }
                let mut b = [0u8; 34];
                if read_up_to(f, &mut b)? < 34 {
                    return Err(corrupt("the STREAMINFO block is truncated"));
                }
                parse_streaminfo(&b, &mut h)?;
            }
            4 if h.vendor.is_none() && h.tags.is_empty() => {
                let mut b = vec![0u8; len as usize];
                let got = read_up_to(f, &mut b)?;
                b.truncate(got);
                if let Some(vc) = vorbis_comment::parse(&b) {
                    h.vendor = vc.vendor;
                    h.tags = vc.tags;
                }
            }
            6 => h.has_picture = true,
            127 => return Err(corrupt("invalid metadata block type 127")),
            _ => {}
        }
        first = false;
        pos = body + len;
        f.seek(SeekFrom::Start(pos))?;
        if last {
            break;
        }
    }
    h.audio_offset = pos;
    Ok(h)
}

/// Parses the metadata blocks of a FLAC file without decoding audio.
pub fn parse_header(path: &Path) -> Result<FlacHeader, AnalysisError> {
    let mut f = File::open(path)?;
    let len = f.metadata()?.len();
    read_metadata(&mut f, len)
}

// ---------------------------------------------------------------------------
// Bit reader

/// The slice ran out before the requested bits (more input is needed).
struct NeedMoreBits;

/// MSB-first bit reader over a byte slice that is followed by at least
/// 8 zero bytes of padding. `end` is the number of valid bits.
struct Bits<'a> {
    buf: &'a [u8],
    pos: usize,
    end: usize,
}

impl Bits<'_> {
    /// The next 64 bits starting at `pos` (at least 57 of them real data or padding).
    #[inline(always)]
    fn peek(&self) -> u64 {
        let i = self.pos >> 3;
        match self.buf.get(i..i + 8) {
            Some(s) => u64::from_be_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]) << (self.pos & 7),
            None => 0,
        }
    }

    /// Reads `n` (1..=57) bits as an unsigned value.
    #[inline(always)]
    fn read(&mut self, n: u32) -> Result<u64, NeedMoreBits> {
        debug_assert!((1..=57).contains(&n));
        let v = self.peek() >> (64 - n);
        self.pos += n as usize;
        if self.pos > self.end {
            return Err(NeedMoreBits);
        }
        Ok(v)
    }

    /// Reads `n` (1..=33) bits as a two's-complement signed value.
    #[inline(always)]
    fn read_signed(&mut self, n: u32) -> Result<i64, NeedMoreBits> {
        let v = self.read(n)?;
        Ok(((v << (64 - n)) as i64) >> (64 - n))
    }

    /// Counts zero bits up to and including the terminating one bit.
    fn unary(&mut self) -> Result<u64, NeedMoreBits> {
        let mut q = 0u64;
        loop {
            let w = self.peek();
            if w != 0 {
                let lz = w.leading_zeros() as usize;
                q += lz as u64;
                self.pos += lz + 1;
                if self.pos > self.end {
                    return Err(NeedMoreBits);
                }
                return Ok(q);
            }
            let avail = 64 - (self.pos & 7);
            q += avail as u64;
            self.pos += avail;
            if self.pos > self.end {
                return Err(NeedMoreBits);
            }
        }
    }

    /// Reads one Rice-coded, zig-zag mapped residual with parameter `k`.
    #[inline(always)]
    fn rice(&mut self, k: u32) -> Result<i64, SubErr> {
        let w = self.peek();
        if w != 0 {
            let lz = w.leading_zeros();
            let used = lz + 1 + k;
            if used <= 64 - (self.pos & 7) as u32 {
                let r = if k == 0 { 0 } else { ((w << lz) << 1) >> (64 - k) };
                self.pos += used as usize;
                if self.pos > self.end {
                    return Err(SubErr::NeedMore);
                }
                let u = ((lz as u64) << k) | r;
                return Ok(((u >> 1) as i64) ^ -((u & 1) as i64));
            }
        }
        self.rice_slow(k)
    }

    #[inline(never)]
    fn rice_slow(&mut self, k: u32) -> Result<i64, SubErr> {
        let q = self.unary()?;
        if q > 1 << 32 {
            return Err(SubErr::Bad("residual out of range"));
        }
        let r = if k == 0 { 0 } else { self.read(k)? };
        let u = (q << k) | r;
        Ok(((u >> 1) as i64) ^ -((u & 1) as i64))
    }
}

// ---------------------------------------------------------------------------
// Frames

/// Subframe-level failure.
enum SubErr {
    NeedMore,
    Bad(&'static str),
}

impl From<NeedMoreBits> for SubErr {
    fn from(_: NeedMoreBits) -> Self {
        SubErr::NeedMore
    }
}

#[derive(Clone, Copy, Debug)]
struct StreamParams {
    channels: usize,
    bps: u32,
    /// STREAMINFO maximum block size (0 when not usable).
    max_block: usize,
}

#[derive(Clone, Copy, Debug)]
struct FrameHeader {
    variable: bool,
    block_size: usize,
    channels: usize,
    assignment: u8,
    bps: u32,
    /// Frame number (fixed blocking) or first sample number (variable blocking).
    number: u64,
    /// Header length in bytes, including the CRC-8.
    len: usize,
}

enum HeaderParse {
    Ok(FrameHeader),
    NeedMore,
    Bad,
}

enum Utf8Num {
    Ok(u64, usize),
    NeedMore,
    Bad,
}

/// Decodes the UTF-8-style coded frame/sample number starting at `at`.
fn read_coded_number(b: &[u8], at: usize) -> Utf8Num {
    let first = match b.get(at) {
        Some(&x) => x,
        None => return Utf8Num::NeedMore,
    };
    let (len, mut v) = if first & 0x80 == 0 {
        (1, first as u64)
    } else if first & 0xE0 == 0xC0 {
        (2, (first & 0x1F) as u64)
    } else if first & 0xF0 == 0xE0 {
        (3, (first & 0x0F) as u64)
    } else if first & 0xF8 == 0xF0 {
        (4, (first & 0x07) as u64)
    } else if first & 0xFC == 0xF8 {
        (5, (first & 0x03) as u64)
    } else if first & 0xFE == 0xFC {
        (6, (first & 0x01) as u64)
    } else if first == 0xFE {
        (7, 0u64)
    } else {
        return Utf8Num::Bad;
    };
    if b.len() < at + len {
        return Utf8Num::NeedMore;
    }
    for &c in &b[at + 1..at + len] {
        if c & 0xC0 != 0x80 {
            return Utf8Num::Bad;
        }
        v = (v << 6) | (c & 0x3F) as u64;
    }
    Utf8Num::Ok(v, at + len)
}

/// Parses and validates a frame header at the start of `b`.
fn parse_frame_header(b: &[u8], p: &StreamParams) -> HeaderParse {
    if b.len() < 2 {
        return HeaderParse::NeedMore;
    }
    if b[0] != 0xFF || (b[1] & 0xFE) != 0xF8 {
        return HeaderParse::Bad;
    }
    if b.len() < 4 {
        return HeaderParse::NeedMore;
    }
    let variable = b[1] & 1 == 1;
    let bs_code = b[2] >> 4;
    let sr_code = b[2] & 0x0F;
    let ch_code = b[3] >> 4;
    let ss_code = (b[3] >> 1) & 0x07;
    if b[3] & 1 != 0 || bs_code == 0 || sr_code == 0x0F || ch_code > 10 || ss_code == 3 {
        return HeaderParse::Bad;
    }
    let (number, mut i) = match read_coded_number(b, 4) {
        Utf8Num::Ok(n, next) => (n, next),
        Utf8Num::NeedMore => return HeaderParse::NeedMore,
        Utf8Num::Bad => return HeaderParse::Bad,
    };
    if !variable && number > 0x7FFF_FFFF {
        return HeaderParse::Bad;
    }
    let block_size = match bs_code {
        1 => 192,
        2..=5 => 576usize << (bs_code - 2),
        6 => {
            if b.len() < i + 1 {
                return HeaderParse::NeedMore;
            }
            i += 1;
            b[i - 1] as usize + 1
        }
        7 => {
            if b.len() < i + 2 {
                return HeaderParse::NeedMore;
            }
            i += 2;
            u16::from_be_bytes([b[i - 2], b[i - 1]]) as usize + 1
        }
        _ => 256usize << (bs_code - 8),
    };
    i += match sr_code {
        12 => 1,
        13 | 14 => 2,
        _ => 0,
    };
    if b.len() < i + 1 {
        return HeaderParse::NeedMore;
    }
    if crc8(&b[..i]) != b[i] {
        return HeaderParse::Bad;
    }
    if block_size > 65535 {
        return HeaderParse::Bad;
    }
    let channels = if ch_code < 8 { ch_code as usize + 1 } else { 2 };
    let bps = match ss_code {
        0 => p.bps,
        1 => 8,
        2 => 12,
        4 => 16,
        5 => 20,
        6 => 24,
        7 => 32,
        _ => return HeaderParse::Bad,
    };
    // A change of channel count or sample size mid-stream is not supported
    // (and is almost always a false sync inside damaged data).
    if channels != p.channels || bps != p.bps {
        return HeaderParse::Bad;
    }
    HeaderParse::Ok(FrameHeader { variable, block_size, channels, assignment: ch_code, bps, number, len: i + 1 })
}

/// Reusable per-frame buffers.
struct Scratch {
    ch: Vec<Vec<i64>>,
    coefs: Vec<i64>,
}

enum FrameError {
    /// More input is needed; `in_body` is true once the header parsed.
    NeedMore { in_body: bool },
    /// No valid frame header here.
    BadHeader,
    /// The header is valid but the frame body is not.
    BadBody { header: FrameHeader, why: &'static str },
}

struct Decoded {
    consumed: usize,
    header: FrameHeader,
    crc_ok: bool,
}

/// Decodes one frame from `buf` (`avail` valid bytes followed by padding)
/// into `s.ch`. Channel decorrelation is applied only when the CRC matches.
fn decode_frame(buf: &[u8], avail: usize, p: &StreamParams, s: &mut Scratch) -> Result<Decoded, FrameError> {
    let hdr = match parse_frame_header(&buf[..avail], p) {
        HeaderParse::Ok(h) => h,
        HeaderParse::NeedMore => return Err(FrameError::NeedMore { in_body: false }),
        HeaderParse::Bad => return Err(FrameError::BadHeader),
    };
    let bs = hdr.block_size;
    for c in s.ch.iter_mut().take(hdr.channels) {
        if c.len() < bs {
            c.resize(bs, 0);
        }
    }
    let mut br = Bits { buf, pos: hdr.len * 8, end: avail * 8 };
    for c in 0..hdr.channels {
        let side = matches!((hdr.assignment, c), (8, 1) | (9, 0) | (10, 1));
        let sbps = hdr.bps + side as u32;
        if let Err(e) = decode_subframe(&mut br, &mut s.ch[c][..bs], sbps, &mut s.coefs) {
            return Err(match e {
                SubErr::NeedMore => FrameError::NeedMore { in_body: true },
                SubErr::Bad(why) => FrameError::BadBody { header: hdr, why },
            });
        }
    }
    br.pos = (br.pos + 7) & !7;
    let crc_pos = br.pos / 8;
    let stored = match br.read(16) {
        Ok(v) => v as u16,
        Err(_) => return Err(FrameError::NeedMore { in_body: true }),
    };
    let crc_ok = crc16(&buf[..crc_pos]) == stored;
    if crc_ok {
        decorrelate(hdr.assignment, &mut s.ch, bs);
    }
    Ok(Decoded { consumed: crc_pos + 2, header: hdr, crc_ok })
}

fn decorrelate(assignment: u8, ch: &mut [Vec<i64>], bs: usize) {
    if assignment < 8 || ch.len() < 2 {
        return;
    }
    let (a, b) = ch.split_at_mut(1);
    let l = &mut a[0][..bs];
    let r = &mut b[0][..bs];
    match assignment {
        // left + side: right = left - side
        8 => {
            for (x, y) in l.iter().zip(r.iter_mut()) {
                *y = x.wrapping_sub(*y);
            }
        }
        // side + right: left = side + right
        9 => {
            for (x, y) in l.iter_mut().zip(r.iter()) {
                *x = x.wrapping_add(*y);
            }
        }
        // mid + side
        10 => {
            for (m, sd) in l.iter_mut().zip(r.iter_mut()) {
                let side = *sd;
                let mid = (*m << 1) | (side & 1);
                *m = mid.wrapping_add(side) >> 1;
                *sd = mid.wrapping_sub(side) >> 1;
            }
        }
        _ => {}
    }
}

fn in_range(s: &[i64], bps: u32) -> bool {
    let hi = (1i64 << (bps - 1)) - 1;
    let lo = -(1i64 << (bps - 1));
    let (mn, mx) = s.iter().fold((0i64, 0i64), |(mn, mx), &x| (mn.min(x), mx.max(x)));
    mn >= lo && mx <= hi
}

fn decode_subframe(br: &mut Bits, out: &mut [i64], sbps: u32, coefs: &mut Vec<i64>) -> Result<(), SubErr> {
    let bs = out.len();
    let h = br.read(8)?;
    if h & 0x80 != 0 {
        return Err(SubErr::Bad("subframe padding bit is set"));
    }
    let kind = ((h >> 1) & 0x3F) as u32;
    let mut wasted = 0u32;
    if h & 1 == 1 {
        let z = br.unary()?;
        if z >= sbps as u64 {
            return Err(SubErr::Bad("wasted bits exceed the sample size"));
        }
        wasted = z as u32 + 1;
        if wasted >= sbps {
            return Err(SubErr::Bad("wasted bits exceed the sample size"));
        }
    }
    let bps = sbps - wasted;
    match kind {
        0 => {
            let v = br.read_signed(bps)?;
            out.fill(v);
        }
        1 => {
            for x in out.iter_mut() {
                *x = br.read_signed(bps)?;
            }
        }
        8..=12 => {
            let order = (kind - 8) as usize;
            if order > bs {
                return Err(SubErr::Bad("predictor order exceeds the block size"));
            }
            for x in out[..order].iter_mut() {
                *x = br.read_signed(bps)?;
            }
            decode_residual(br, out, order)?;
            restore_fixed(out, order);
        }
        32..=63 => {
            let order = (kind - 31) as usize;
            if order > bs {
                return Err(SubErr::Bad("predictor order exceeds the block size"));
            }
            for x in out[..order].iter_mut() {
                *x = br.read_signed(bps)?;
            }
            let precision = br.read(4)? as u32 + 1;
            if precision == 16 {
                return Err(SubErr::Bad("invalid LPC coefficient precision"));
            }
            let shift = br.read(5)? as i32;
            let shift = if shift >= 16 { shift - 32 } else { shift };
            if shift < 0 {
                return Err(SubErr::Bad("negative LPC shift"));
            }
            coefs.clear();
            for _ in 0..order {
                coefs.push(br.read_signed(precision)?);
            }
            decode_residual(br, out, order)?;
            restore_lpc(out, coefs, shift as u32);
        }
        _ => return Err(SubErr::Bad("reserved subframe type")),
    }
    if !in_range(out, bps) {
        return Err(SubErr::Bad("decoded samples exceed the sample size"));
    }
    if wasted > 0 {
        for x in out.iter_mut() {
            *x <<= wasted;
        }
    }
    Ok(())
}

fn decode_residual(br: &mut Bits, out: &mut [i64], order: usize) -> Result<(), SubErr> {
    let bs = out.len();
    let method = br.read(2)?;
    if method > 1 {
        return Err(SubErr::Bad("reserved residual coding method"));
    }
    let pbits = if method == 0 { 4 } else { 5 };
    let escape = (1u64 << pbits) - 1;
    let porder = br.read(4)? as u32;
    let parts = 1usize << porder;
    let psize = bs >> porder;
    if psize << porder != bs || psize < order {
        return Err(SubErr::Bad("invalid residual partition order"));
    }
    let mut i = order;
    for part in 0..parts {
        let k = br.read(pbits)?;
        let n = if part == 0 { psize - order } else { psize };
        let dst = &mut out[i..i + n];
        if k == escape {
            let nb = br.read(5)? as u32;
            if nb == 0 {
                dst.fill(0);
            } else {
                for x in dst.iter_mut() {
                    *x = br.read_signed(nb)?;
                }
            }
        } else {
            let k = k as u32;
            for x in dst.iter_mut() {
                *x = br.rice(k)?;
            }
        }
        i += n;
    }
    Ok(())
}

/// Reconstructs samples from residuals with a fixed polynomial predictor.
/// `s[..order]` holds warm-up samples, `s[order..]` residuals.
fn restore_fixed(s: &mut [i64], order: usize) {
    let n = s.len();
    match order {
        1 => {
            for i in 1..n {
                s[i] = s[i].wrapping_add(s[i - 1]);
            }
        }
        2 => {
            for i in 2..n {
                let p = s[i - 1].wrapping_mul(2).wrapping_sub(s[i - 2]);
                s[i] = s[i].wrapping_add(p);
            }
        }
        3 => {
            for i in 3..n {
                let p = s[i - 1].wrapping_sub(s[i - 2]).wrapping_mul(3).wrapping_add(s[i - 3]);
                s[i] = s[i].wrapping_add(p);
            }
        }
        4 => {
            for i in 4..n {
                let p = s[i - 1]
                    .wrapping_add(s[i - 3])
                    .wrapping_mul(4)
                    .wrapping_sub(s[i - 2].wrapping_mul(6))
                    .wrapping_sub(s[i - 4]);
                s[i] = s[i].wrapping_add(p);
            }
        }
        _ => {}
    }
}

/// Reconstructs samples with a quantised LPC predictor (`coefs[j]` weights
/// `s[i - 1 - j]`).
fn restore_lpc(s: &mut [i64], coefs: &[i64], shift: u32) {
    let order = coefs.len();
    let mut rc = [0i64; 32];
    for (j, &c) in coefs.iter().enumerate() {
        rc[order - 1 - j] = c;
    }
    let rc = &rc[..order];
    for i in order..s.len() {
        let mut sum = 0i64;
        for (c, x) in rc.iter().zip(&s[i - order..i]) {
            sum = sum.wrapping_add(c.wrapping_mul(*x));
        }
        s[i] = s[i].wrapping_add(sum >> shift);
    }
}

// ---------------------------------------------------------------------------
// Streaming source

pub struct FlacSource {
    file: File,
    header: FlacHeader,
    info: StreamInfo,
    params: StreamParams,
    buf: Vec<u8>,
    start: usize,
    end: usize,
    eof: bool,
    base_need: usize,
    need: usize,
    scratch: Scratch,
    frame_len: usize,
    frame_pos: usize,
    md5: Md5,
    md5_buf: Vec<u8>,
    or_mask: u32,
    decoded: u64,
    errors: u32,
    messages: Vec<String>,
    truncated: bool,
    finished: bool,
    last_number: Option<u64>,
    variable: Option<bool>,
    resyncing: bool,
    scale: f64,
}

impl FlacSource {
    pub fn open(path: &Path) -> Result<Self, OpenError> {
        let io_err = |e: std::io::Error| OpenError::Failed(AnalysisError::Io(e.to_string()));
        let mut file = File::open(path).map_err(io_err)?;
        let len = file.metadata().map_err(io_err)?.len();
        let header = read_metadata(&mut file, len).map_err(OpenError::Failed)?;
        if header.bits_per_sample < 4 {
            return Err(OpenError::NotNative(format!("{}-bit FLAC is not supported natively", header.bits_per_sample)));
        }
        file.seek(SeekFrom::Start(header.audio_offset)).map_err(io_err)?;
        let channels = header.channels as usize;
        let bps = header.bits_per_sample as u32;
        let max_block = if header.max_block_size >= 16 { header.max_block_size as usize } else { 0 };
        let worst_block = if max_block > 0 { max_block } else { 65535 };
        let verbatim_bytes = worst_block * channels * (bps as usize + 1) / 8 + 64;
        let base_need = if header.max_frame_size > 0 { header.max_frame_size as usize + 16 } else { verbatim_bytes };
        let base_need = base_need.clamp(64, MAX_FRAME_BYTES);
        let info = StreamInfo {
            sample_rate: header.sample_rate,
            channels: header.channels,
            kind: SampleKind::Int,
            bits: Some(header.bits_per_sample),
            frames_hint: header.total_frames,
            decoder: "native-flac",
        };
        Ok(FlacSource {
            file,
            info,
            params: StreamParams { channels, bps, max_block },
            buf: vec![0u8; READ_CHUNK + PAD],
            start: 0,
            end: 0,
            eof: false,
            base_need,
            need: base_need,
            scratch: Scratch { ch: vec![Vec::new(); channels], coefs: Vec::with_capacity(32) },
            frame_len: 0,
            frame_pos: 0,
            md5: Md5::new(),
            md5_buf: Vec::new(),
            or_mask: 0,
            decoded: 0,
            errors: 0,
            messages: Vec::new(),
            truncated: false,
            finished: false,
            last_number: None,
            variable: None,
            resyncing: false,
            scale: 1.0 / (1u64 << (bps - 1)) as f64,
            header,
        })
    }

    pub fn header(&self) -> &FlacHeader {
        &self.header
    }

    fn note(&mut self, msg: String) {
        if self.messages.len() < MAX_MESSAGES && !self.messages.contains(&msg) {
            self.messages.push(msg);
        }
    }

    fn error(&mut self, msg: String) {
        self.errors += 1;
        self.note(msg);
    }

    /// Ensures at least `want` bytes are buffered after `start` (unless EOF),
    /// keeping `PAD` zero bytes after the valid data.
    fn fill(&mut self, want: usize) -> Result<(), AnalysisError> {
        if self.eof || self.end - self.start >= want {
            return Ok(());
        }
        if self.start > 0 {
            self.buf.copy_within(self.start..self.end, 0);
            self.end -= self.start;
            self.start = 0;
        }
        let target = want.max(self.end + READ_CHUNK) + PAD;
        if self.buf.len() < target {
            self.buf.resize(target, 0);
        }
        while self.end < want && !self.eof {
            let limit = self.buf.len() - PAD;
            match self.file.read(&mut self.buf[self.end..limit]) {
                Ok(0) => self.eof = true,
                Ok(n) => self.end += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(AnalysisError::Io(format!("reading FLAC data: {e}"))),
            }
        }
        let pad_end = self.end + PAD;
        self.buf[self.end..pad_end].fill(0);
        Ok(())
    }

    fn plausible(&self, h: &FrameHeader) -> bool {
        if let Some(v) = self.variable {
            if v != h.variable {
                return false;
            }
        }
        if self.params.max_block > 0 && h.block_size > self.params.max_block {
            return false;
        }
        if let Some(last) = self.last_number {
            if h.number <= last {
                return false;
            }
        }
        true
    }

    /// Scans forward from `from` for the next plausible frame header and moves
    /// `start` there. Returns false when the stream holds no further frame.
    fn resync(&mut self, from: usize) -> Result<bool, AnalysisError> {
        let mut p = from.min(self.end);
        loop {
            let mut need_more = false;
            while p + 1 < self.end {
                if self.buf[p] == 0xFF && (self.buf[p + 1] & 0xFE) == 0xF8 {
                    match parse_frame_header(&self.buf[p..self.end], &self.params) {
                        HeaderParse::Ok(h) if self.plausible(&h) => {
                            self.start = p;
                            return Ok(true);
                        }
                        HeaderParse::NeedMore if !self.eof => {
                            need_more = true;
                            break;
                        }
                        _ => {}
                    }
                }
                p += 1;
            }
            if self.eof {
                self.start = self.end;
                return Ok(false);
            }
            // Everything before `p` is known not to start a frame.
            self.start = if need_more { p } else { p.min(self.end) };
            let have = self.end - self.start;
            self.fill(have + READ_CHUNK)?;
            p = self.start;
        }
    }

    /// Decodes the next frame into the scratch buffers. Returns false at the
    /// end of the stream.
    fn decode_next(&mut self) -> Result<bool, AnalysisError> {
        loop {
            if let Some(total) = self.header.total_frames {
                if self.decoded >= total {
                    return Ok(false);
                }
            }
            self.fill(self.need)?;
            let avail = self.end - self.start;
            if avail == 0 {
                return Ok(false);
            }
            let res = decode_frame(&self.buf[self.start..self.end + PAD], avail, &self.params, &mut self.scratch);
            match res {
                Ok(d) => {
                    if !d.crc_ok {
                        self.error("Frame checksum (CRC-16) mismatch; the frame was replaced with silence".into());
                        self.zero_frame(d.header.block_size);
                    }
                    self.resyncing = false;
                    self.variable = Some(d.header.variable);
                    self.last_number = Some(d.header.number);
                    self.start += d.consumed;
                    self.need = self.base_need;
                    self.finish_frame(d.header.block_size);
                    return Ok(true);
                }
                Err(FrameError::NeedMore { in_body }) => {
                    if !self.eof {
                        if avail >= MAX_FRAME_BYTES {
                            self.error("Implausibly large frame; skipped".into());
                            self.resyncing = true;
                            if !self.resync(self.start + 2)? {
                                return Ok(false);
                            }
                            continue;
                        }
                        self.need = (self.need * 2).max(avail + 1).min(MAX_FRAME_BYTES);
                        continue;
                    }
                    if in_body {
                        self.truncated = true;
                        self.note("The file ends in the middle of an audio frame".into());
                    }
                    self.start = self.end;
                    return Ok(false);
                }
                Err(FrameError::BadHeader) => {
                    let was_resyncing = self.resyncing;
                    if !self.resync(self.start + 1)? {
                        return Ok(false);
                    }
                    if !was_resyncing {
                        self.error("Lost frame sync; skipped damaged data".into());
                    }
                    self.resyncing = true;
                }
                Err(FrameError::BadBody { header, why }) => {
                    if self.resyncing {
                        // Probably a false sync inside damaged data: keep scanning.
                        if !self.resync(self.start + 2)? {
                            return Ok(false);
                        }
                        continue;
                    }
                    self.error(format!("Damaged frame ({why}); replaced with silence"));
                    self.resyncing = true;
                    self.variable = Some(header.variable);
                    self.last_number = Some(header.number);
                    self.zero_frame(header.block_size);
                    let more = self.resync(self.start + 2)?;
                    if !more {
                        self.start = self.end;
                    }
                    self.finish_frame(header.block_size);
                    return Ok(true);
                }
            }
        }
    }

    fn zero_frame(&mut self, bs: usize) {
        for c in self.scratch.ch.iter_mut() {
            if c.len() < bs {
                c.resize(bs, 0);
            }
            c[..bs].fill(0);
        }
    }

    /// Feeds the decoded block to the MD5 and bit-depth accumulators and
    /// makes it available to `read`.
    fn finish_frame(&mut self, bs: usize) {
        let ch = self.params.channels;
        let bps = self.params.bps;
        let bytes = bps.div_ceil(8) as usize;
        self.md5_buf.resize(bs * ch * bytes, 0);
        let mut or_all: u64 = 0;
        let chans = &self.scratch.ch;
        let out = &mut self.md5_buf;
        // Interleave as little-endian, ceil(bps/8) bytes per sample (the
        // layout libFLAC hashes). Fixed-width paths avoid per-sample memcpy.
        macro_rules! pack {
            ($n:literal) => {{
                let mut cells = out.as_chunks_mut::<$n>().0.iter_mut();
                for i in 0..bs {
                    for c in chans.iter().take(ch) {
                        let s = c[i];
                        or_all |= s as u64;
                        let v = (s as u32).to_le_bytes();
                        if let Some(cell) = cells.next() {
                            cell.copy_from_slice(&v[..$n]);
                        }
                    }
                }
            }};
        }
        match bytes {
            1 => pack!(1),
            2 => pack!(2),
            3 => pack!(3),
            _ => pack!(4),
        }
        self.md5.update(&self.md5_buf);
        self.or_mask |= (or_all as u32) << (32 - bps);
        self.decoded += bs as u64;
        self.frame_len = bs;
        self.frame_pos = 0;
    }
}

impl PcmSource for FlacSource {
    fn info(&self) -> &StreamInfo {
        &self.info
    }

    fn read(&mut self, out: &mut Vec<f32>, max_frames: usize) -> Result<usize, AnalysisError> {
        out.clear();
        let ch = self.params.channels;
        out.reserve(max_frames * ch);
        let mut produced = 0;
        while produced < max_frames {
            if self.frame_pos >= self.frame_len {
                if self.finished {
                    break;
                }
                if !self.decode_next()? {
                    self.finished = true;
                    break;
                }
                continue;
            }
            let n = (max_frames - produced).min(self.frame_len - self.frame_pos);
            let scale = self.scale;
            let (from, to) = (self.frame_pos, self.frame_pos + n);
            if ch == 2 {
                let (l, r) = (&self.scratch.ch[0][from..to], &self.scratch.ch[1][from..to]);
                for (a, b) in l.iter().zip(r) {
                    out.push((*a as f64 * scale) as f32);
                    out.push((*b as f64 * scale) as f32);
                }
            } else {
                for i in from..to {
                    for c in self.scratch.ch.iter().take(ch) {
                        out.push((c[i] as f64 * scale) as f32);
                    }
                }
            }
            self.frame_pos = to;
            produced += n;
            if self.frame_pos >= self.frame_len {
                if let Some(total) = self.header.total_frames {
                    if self.decoded >= total {
                        self.finished = true;
                    }
                }
            }
        }
        Ok(produced)
    }

    fn finish(&mut self) -> DecodeStats {
        if self.finished {
            if let Some(total) = self.header.total_frames {
                if self.decoded < total {
                    self.truncated = true;
                    let msg = format!("Decoded {} of {} declared samples per channel", self.decoded, total);
                    self.note(msg);
                }
            }
        }
        let checksum = match self.header.md5 {
            None => ChecksumStatus::NotStored,
            Some(_) if !self.finished => ChecksumStatus::NotChecked,
            Some(expected) => {
                if self.md5.digest() == expected {
                    ChecksumStatus::Verified
                } else {
                    ChecksumStatus::Mismatch
                }
            }
        };
        DecodeStats {
            int_or_mask: Some(self.or_mask),
            decode_errors: self.errors,
            truncated: self.truncated,
            messages: self.messages.clone(),
            checksum,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_check_values() {
        // CRC-8/SMBUS and CRC-16/UMTS (a.k.a. BUYPASS) check values.
        assert_eq!(crc8(b"123456789"), 0xF4);
        assert_eq!(crc16(b"123456789"), 0xFEE8);
    }

    #[test]
    fn coded_numbers() {
        for &n in &[0u64, 1, 0x7F, 0x80, 0x7FF, 0x800, 0xFFFF, 0x10000, 0x1F_FFFF, 0x3FF_FFFF, 0x7FFF_FFFF, 0xF_FFFF_FFFF] {
            let enc = test_encoder::encode_number(n);
            match read_coded_number(&enc, 0) {
                Utf8Num::Ok(v, next) => {
                    assert_eq!(v, n);
                    assert_eq!(next, enc.len());
                }
                _ => panic!("failed to decode {n}"),
            }
            if enc.len() > 1 {
                assert!(matches!(read_coded_number(&enc[..enc.len() - 1], 0), Utf8Num::NeedMore));
            }
        }
        assert!(matches!(read_coded_number(&[0x80], 0), Utf8Num::Bad));
        assert!(matches!(read_coded_number(&[0xFF, 0x80], 0), Utf8Num::Bad));
        assert!(matches!(read_coded_number(&[0xC2, 0x41], 0), Utf8Num::Bad));
    }

    #[test]
    fn bit_reader_rice_and_signed() {
        let mut w = test_encoder::BitWriter::default();
        let values: Vec<i64> = (-300..300).map(|v| v * 7919 % 4001).collect();
        for &k in &[0u32, 1, 4, 13, 30] {
            for &v in &values {
                w.rice(v, k);
            }
        }
        w.signed(-5, 33);
        w.signed((1i64 << 32) - 1, 33);
        w.signed(-(1i64 << 32), 33);
        w.align();
        let mut bytes = w.bytes;
        let end = bytes.len() * 8;
        bytes.extend_from_slice(&[0u8; PAD]);
        let mut br = Bits { buf: &bytes, pos: 0, end };
        for &k in &[0u32, 1, 4, 13, 30] {
            for &v in &values {
                match br.rice(k) {
                    Ok(x) => assert_eq!(x, v, "k={k}"),
                    Err(_) => panic!("rice decode failed"),
                }
            }
        }
        assert_eq!(br.read_signed(33).ok(), Some(-5));
        assert_eq!(br.read_signed(33).ok(), Some((1i64 << 32) - 1));
        assert_eq!(br.read_signed(33).ok(), Some(-(1i64 << 32)));
        // Reading past the end reports NeedMore instead of panicking.
        br.pos = br.end;
        assert!(br.read(8).is_err());
        assert!(br.unary().is_err());
    }

    fn decode_file(path: &Path, chunk: usize) -> (Vec<f32>, DecodeStats, StreamInfo) {
        let mut src = match FlacSource::open(path) {
            Ok(s) => s,
            Err(OpenError::NotNative(m)) => panic!("not native: {m}"),
            Err(OpenError::Failed(e)) => panic!("failed: {e}"),
        };
        let info = src.info().clone();
        let mut all = Vec::new();
        let mut buf = Vec::new();
        loop {
            let n = src.read(&mut buf, chunk).expect("read");
            if n == 0 {
                break;
            }
            all.extend_from_slice(&buf);
        }
        (all, src.finish(), info)
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("sonora-flac-unit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    fn signal(frames: usize, ch: usize, bps: u32, seed: u64) -> Vec<Vec<i64>> {
        let amp = ((1i64 << (bps - 1)) - 1) as f64;
        let mut state = seed;
        (0..ch)
            .map(|c| {
                (0..frames)
                    .map(|i| {
                        state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
                        let noise = ((state >> 33) as f64 / (1u64 << 31) as f64 - 0.5) * 0.05;
                        let t = i as f64;
                        let v = 0.6 * (t * 0.031 * (c as f64 + 1.0)).sin() + 0.3 * (t * 0.0017).sin() + noise;
                        (v.clamp(-1.0, 1.0) * amp).round() as i64
                    })
                    .collect()
            })
            .collect()
    }

    /// Makes the blocks that the plan codes as CONSTANT actually constant
    /// (for stereo modes the side channel is made constant by setting R = L).
    fn enforce_constants(input: &mut [Vec<i64>], plan: &[test_encoder::FramePlan]) {
        let mut pos = 0;
        for p in plan {
            for (c, sub) in p.subs.iter().enumerate() {
                if !matches!(sub, test_encoder::Sub::Constant) {
                    continue;
                }
                if p.assignment >= 8 {
                    let (l, r) = input.split_at_mut(1);
                    r[0][pos..pos + p.block].copy_from_slice(&l[0][pos..pos + p.block]);
                } else {
                    let v = input[c][pos];
                    input[c][pos..pos + p.block].fill(v);
                }
            }
            pos += p.block;
        }
    }

    fn check_roundtrip(name: &str, bps: u32, ch: usize, frames: usize, plan: &[test_encoder::FramePlan], variable: bool) {
        let mut input = signal(frames, ch, bps, 42 + bps as u64 + ch as u64);
        enforce_constants(&mut input, plan);
        let bytes = test_encoder::encode(&input, bps, 44_100, plan, variable);
        let path = tmp(name);
        std::fs::write(&path, &bytes).unwrap();
        let h = parse_header(&path).unwrap();
        assert_eq!(h.bits_per_sample as u32, bps);
        assert_eq!(h.channels as usize, ch);
        assert_eq!(h.total_frames, Some(frames as u64));
        assert_eq!(h.vendor.as_deref(), Some("sonora test encoder"));
        for chunk in [1usize, 777, 100_000] {
            let (out, stats, info) = decode_file(&path, chunk);
            assert_eq!(info.bits, Some(bps as u16));
            assert_eq!(stats.decode_errors, 0, "{name}: {:?}", stats.messages);
            assert_eq!(stats.checksum, ChecksumStatus::Verified, "{name}");
            assert!(!stats.truncated);
            assert_eq!(out.len(), frames * ch, "{name}");
            if bps <= 24 {
                let scale = (1u64 << (bps - 1)) as f64;
                for i in 0..frames {
                    for c in 0..ch {
                        let got = (out[i * ch + c] as f64 * scale).round() as i64;
                        assert_eq!(got, input[c][i], "{name} frame {i} ch {c}");
                    }
                }
            }
        }
    }

    #[test]
    fn all_subframe_types_and_channel_modes() {
        use test_encoder::{FramePlan as P, Sub};
        let plan = [
            P { block: 1152, assignment: 0, subs: vec![Sub::Constant] },
            P { block: 1152, assignment: 0, subs: vec![Sub::Verbatim] },
            P { block: 1024, assignment: 0, subs: vec![Sub::Fixed { order: 0, porder: 0, method5: false }] },
            P { block: 1024, assignment: 0, subs: vec![Sub::Fixed { order: 1, porder: 2, method5: false }] },
            P { block: 1024, assignment: 0, subs: vec![Sub::Fixed { order: 2, porder: 3, method5: true }] },
            P { block: 1024, assignment: 0, subs: vec![Sub::Fixed { order: 3, porder: 1, method5: false }] },
            P { block: 1024, assignment: 0, subs: vec![Sub::Fixed { order: 4, porder: 4, method5: false }] },
            P { block: 2048, assignment: 0, subs: vec![Sub::Lpc { order: 8, precision: 12, shift: 9, porder: 3 }] },
            P { block: 2048, assignment: 0, subs: vec![Sub::Lpc { order: 32, precision: 15, shift: 12, porder: 2 }] },
            P { block: 300, assignment: 0, subs: vec![Sub::Lpc { order: 1, precision: 4, shift: 2, porder: 0 }] },
        ];
        check_roundtrip("mono16.flac", 16, 1, plan.iter().map(|p| p.block).sum(), &plan, false);
        check_roundtrip("mono8.flac", 8, 1, plan.iter().map(|p| p.block).sum(), &plan, false);
        check_roundtrip("mono20.flac", 20, 1, plan.iter().map(|p| p.block).sum(), &plan, true);

        let fx = |order| Sub::Fixed { order, porder: 2, method5: false };
        let stereo = [
            P { block: 4096, assignment: 0, subs: vec![fx(2), fx(1)] },
            P { block: 4096, assignment: 8, subs: vec![fx(2), fx(2)] },
            P { block: 4096, assignment: 9, subs: vec![Sub::Verbatim, fx(3)] },
            P { block: 4096, assignment: 10, subs: vec![Sub::Lpc { order: 12, precision: 14, shift: 11, porder: 4 }, fx(2)] },
            P { block: 17, assignment: 10, subs: vec![Sub::Verbatim, Sub::Constant] },
        ];
        let frames: usize = stereo.iter().map(|p| p.block).sum();
        check_roundtrip("stereo16.flac", 16, 2, frames, &stereo, false);
        check_roundtrip("stereo24.flac", 24, 2, frames, &stereo, true);
        // 32-bit audio: the side channel needs 33 bits.
        check_roundtrip("stereo32.flac", 32, 2, frames, &stereo, false);
    }

    #[test]
    fn wasted_bits() {
        use test_encoder::{FramePlan as P, Sub};
        let frames = 4096;
        let mut input = signal(frames, 2, 24, 9);
        for ch in input.iter_mut() {
            for s in ch.iter_mut() {
                *s = (*s >> 8) << 8; // 16-bit content in a 24-bit stream
            }
        }
        let plan = [P { block: 4096, assignment: 10, subs: vec![Sub::Fixed { order: 2, porder: 2, method5: false }; 2] }];
        let bytes = test_encoder::encode_with_wasted(&input, 24, 48_000, &plan, 8);
        let path = tmp("wasted.flac");
        std::fs::write(&path, &bytes).unwrap();
        let (out, stats, _) = decode_file(&path, 1000);
        assert_eq!(stats.checksum, ChecksumStatus::Verified);
        assert_eq!(stats.decode_errors, 0);
        // 16 significant bits: the low 16 bits of the left-aligned mask are zero.
        assert_eq!(stats.int_or_mask.unwrap() & 0xFFFF, 0);
        assert_ne!(stats.int_or_mask.unwrap() & 0x10000, 0);
        let scale = (1u64 << 23) as f64;
        for i in 0..frames {
            assert_eq!((out[2 * i] as f64 * scale).round() as i64, input[0][i]);
            assert_eq!((out[2 * i + 1] as f64 * scale).round() as i64, input[1][i]);
        }
    }

    #[test]
    fn damaged_streams_never_panic() {
        use test_encoder::{FramePlan as P, Sub};
        let fx = |order| Sub::Fixed { order, porder: 2, method5: false };
        let plan: Vec<P> = (0..12).map(|i| P { block: 1024, assignment: (i % 3) as u8 + 8, subs: vec![fx(2), fx(1)] }).collect();
        let input = signal(12 * 1024, 2, 16, 77);
        let clean = test_encoder::encode(&input, 16, 44_100, &plan, false);
        let path = tmp("fuzz.flac");
        let mut state = 0x1234_5678_9ABC_DEF0u64;
        let mut rnd = |n: usize| {
            state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            ((state >> 33) as usize) % n.max(1)
        };
        for round in 0..300 {
            let mut bytes = clean.clone();
            match round % 4 {
                0 => {
                    for _ in 0..1 + rnd(8) {
                        let i = rnd(bytes.len());
                        bytes[i] ^= 1 << rnd(8);
                    }
                }
                1 => {
                    let cut = rnd(bytes.len());
                    bytes.truncate(cut);
                }
                2 => {
                    let i = rnd(bytes.len());
                    let n = rnd(200).min(bytes.len() - i);
                    for b in &mut bytes[i..i + n] {
                        *b = rnd(256) as u8;
                    }
                }
                _ => {
                    // Damage only the audio region, keep metadata intact.
                    let lo = 4 + 38 + 30;
                    for _ in 0..1 + rnd(4) {
                        let i = lo + rnd(bytes.len() - lo);
                        bytes[i] = rnd(256) as u8;
                    }
                }
            }
            std::fs::write(&path, &bytes).unwrap();
            let _ = parse_header(&path);
            if let Ok(mut src) = FlacSource::open(&path) {
                let mut buf = Vec::new();
                let mut total = 0usize;
                while let Ok(n) = src.read(&mut buf, 4096) {
                    if n == 0 {
                        break;
                    }
                    total += n;
                    assert!(total < 100 * 12 * 1024, "runaway output");
                }
                let _ = src.finish();
            }
        }
    }

    #[test]
    fn corrupted_frame_is_detected_and_silenced() {
        use test_encoder::{FramePlan as P, Sub};
        let fx = |order| Sub::Fixed { order, porder: 2, method5: false };
        let plan: Vec<P> = (0..8).map(|_| P { block: 2048, assignment: 10, subs: vec![fx(2), fx(2)] }).collect();
        let input = signal(8 * 2048, 2, 16, 5);
        let mut bytes = test_encoder::encode(&input, 16, 44_100, &plan, false);
        let mid = bytes.len() / 2;
        for b in &mut bytes[mid..mid + 8] {
            *b ^= 0x5A;
        }
        let path = tmp("corrupt.flac");
        std::fs::write(&path, &bytes).unwrap();
        let (out, stats, _) = decode_file(&path, 4096);
        assert!(stats.decode_errors >= 1, "{:?}", stats.messages);
        assert_eq!(stats.checksum, ChecksumStatus::Mismatch);
        assert!(!stats.messages.is_empty());
        // The timeline is preserved: at most one block is lost.
        assert!(out.len() >= 7 * 2048 * 2, "decoded {} samples", out.len());
    }

    #[test]
    fn truncated_and_not_flac() {
        use test_encoder::{FramePlan as P, Sub};
        let plan: Vec<P> = (0..6).map(|_| P { block: 1024, assignment: 0, subs: vec![Sub::Verbatim] }).collect();
        let input = signal(6 * 1024, 1, 16, 1);
        let bytes = test_encoder::encode(&input, 16, 44_100, &plan, false);
        let path = tmp("trunc.flac");
        std::fs::write(&path, &bytes[..bytes.len() * 2 / 3]).unwrap();
        let (out, stats, _) = decode_file(&path, 1000);
        assert!(stats.truncated);
        assert_eq!(stats.checksum, ChecksumStatus::Mismatch);
        assert!(out.len() < 6 * 1024);

        let junk = tmp("junk.flac");
        std::fs::write(&junk, b"this is definitely not a FLAC file, just text").unwrap();
        assert!(parse_header(&junk).is_err());
        assert!(matches!(FlacSource::open(&junk), Err(OpenError::Failed(_))));

        // Early stop: checksum cannot be compared.
        let full = tmp("full.flac");
        std::fs::write(&full, &bytes).unwrap();
        let mut src = FlacSource::open(&full).ok().unwrap();
        let mut buf = Vec::new();
        src.read(&mut buf, 100).unwrap();
        assert_eq!(src.finish().checksum, ChecksumStatus::NotChecked);
    }

    /// A tiny FLAC encoder used only to produce test streams that exercise
    /// every subframe type, residual coding mode and channel assignment.
    pub(super) mod test_encoder {
        use crate::md5::Md5;
        use crate::meta::vorbis_comment;

        #[derive(Default)]
        pub struct BitWriter {
            pub bytes: Vec<u8>,
            acc: u64,
            n: u32,
        }

        impl BitWriter {
            pub fn bits(&mut self, v: u64, n: u32) {
                for i in (0..n).rev() {
                    self.acc = (self.acc << 1) | ((v >> i) & 1);
                    self.n += 1;
                    if self.n == 8 {
                        self.bytes.push(self.acc as u8);
                        self.acc = 0;
                        self.n = 0;
                    }
                }
            }
            pub fn signed(&mut self, v: i64, n: u32) {
                self.bits((v as u64) & ((1u64 << n) - 1), n);
            }
            pub fn unary(&mut self, q: u64) {
                for _ in 0..q {
                    self.bits(0, 1);
                }
                self.bits(1, 1);
            }
            pub fn rice(&mut self, v: i64, k: u32) {
                let u = ((v << 1) ^ (v >> 63)) as u64;
                self.unary(u >> k);
                if k > 0 {
                    self.bits(u & ((1u64 << k) - 1), k);
                }
            }
            pub fn align(&mut self) {
                while self.n != 0 {
                    self.bits(0, 1);
                }
            }
        }

        #[derive(Clone, Copy, Debug)]
        pub enum Sub {
            Constant,
            Verbatim,
            Fixed { order: usize, porder: u32, method5: bool },
            Lpc { order: usize, precision: u32, shift: u32, porder: u32 },
        }

        #[derive(Clone, Debug)]
        pub struct FramePlan {
            pub block: usize,
            pub assignment: u8,
            pub subs: Vec<Sub>,
        }

        pub fn encode_number(n: u64) -> Vec<u8> {
            if n < 0x80 {
                return vec![n as u8];
            }
            let len = match n {
                0..=0x7FF => 2,
                0x800..=0xFFFF => 3,
                0x1_0000..=0x1F_FFFF => 4,
                0x20_0000..=0x3FF_FFFF => 5,
                0x400_0000..=0x7FFF_FFFF => 6,
                _ => 7,
            };
            let mut out = vec![0u8; len];
            let mut v = n;
            for i in (1..len).rev() {
                out[i] = 0x80 | (v & 0x3F) as u8;
                v >>= 6;
            }
            out[0] = if len == 7 { 0xFE } else { ((0xFF00u16 >> len) as u8) | v as u8 };
            out
        }

        fn residual(w: &mut BitWriter, res: &[i64], order: usize, porder: u32, method5: bool) {
            let block = res.len();
            let parts = 1usize << porder;
            let psize = block >> porder;
            let rice_k = |chunk: &[i64]| {
                let mean = chunk.iter().map(|v| v.unsigned_abs()).sum::<u64>() / chunk.len().max(1) as u64;
                64 - mean.leading_zeros()
            };
            // Large residuals need the 5-bit parameter method.
            let mut i = order;
            let mut need5 = method5;
            for p in 0..parts {
                let n = if p == 0 { psize - order } else { psize };
                if rice_k(&res[i..i + n]) > 14 {
                    need5 = true;
                }
                i += n;
            }
            let (pbits, escape, kmax) = if need5 { (5, 31u64, 30) } else { (4, 15u64, 14) };
            w.bits(need5 as u64, 2);
            w.bits(porder as u64, 4);
            let mut i = order;
            for p in 0..parts {
                let n = if p == 0 { psize - order } else { psize };
                let chunk = &res[i..i + n];
                let max = chunk.iter().map(|v| v.unsigned_abs()).max().unwrap_or(0);
                let nb = if max == 0 { 0 } else { 65 - max.leading_zeros() };
                // Alternate between escaped (raw) partitions and Rice ones.
                if p % 3 == 2 && nb <= 31 {
                    w.bits(escape, pbits);
                    w.bits(nb as u64, 5);
                    if nb > 0 {
                        for &v in chunk {
                            w.signed(v, nb);
                        }
                    }
                } else {
                    let k = rice_k(chunk).min(kmax);
                    w.bits(k as u64, pbits);
                    for &v in chunk {
                        w.rice(v, k);
                    }
                }
                i += n;
            }
        }

        fn fixed_residual(x: &[i64], order: usize) -> Vec<i64> {
            let mut r = x.to_vec();
            for i in order..x.len() {
                let p = match order {
                    0 => 0,
                    1 => x[i - 1],
                    2 => 2 * x[i - 1] - x[i - 2],
                    3 => 3 * x[i - 1] - 3 * x[i - 2] + x[i - 3],
                    _ => 4 * x[i - 1] - 6 * x[i - 2] + 4 * x[i - 3] - x[i - 4],
                };
                r[i] = x[i] - p;
            }
            r
        }

        fn subframe(w: &mut BitWriter, x: &[i64], bps: u32, sub: Sub, wasted: u32) {
            let bps = bps - wasted;
            let x: Vec<i64> = x.iter().map(|v| v >> wasted).collect();
            let kind = match sub {
                Sub::Constant => 0,
                Sub::Verbatim => 1,
                Sub::Fixed { order, .. } => 8 + order as u64,
                Sub::Lpc { order, .. } => 31 + order as u64,
            };
            w.bits(kind << 1 | (wasted > 0) as u64, 8);
            if wasted > 0 {
                w.unary(wasted as u64 - 1);
            }
            match sub {
                Sub::Constant => w.signed(x[0], bps),
                Sub::Verbatim => {
                    for &v in &x {
                        w.signed(v, bps);
                    }
                }
                Sub::Fixed { order, porder, method5 } => {
                    for &v in &x[..order] {
                        w.signed(v, bps);
                    }
                    residual(w, &fixed_residual(&x, order), order, porder, method5);
                }
                Sub::Lpc { order, precision, shift, porder } => {
                    for &v in &x[..order] {
                        w.signed(v, bps);
                    }
                    // A good predictor (second difference, or first difference
                    // for order 1) plus small extra taps that exercise the
                    // full-order arithmetic.
                    let limit = (1i64 << (precision - 1)) - 1;
                    let unit = 1i64 << shift;
                    let coefs: Vec<i64> = (0..order)
                        .map(|j| match (order, j) {
                            (1, 0) => unit,
                            (_, 0) => 2 * unit,
                            (_, 1) => -unit,
                            _ => ((j as i64 * 7) % 5) - 2,
                        })
                        .collect();
                    assert!(coefs.iter().all(|c| *c <= limit && *c >= -limit - 1), "test coefficients exceed precision");
                    w.bits(precision as u64 - 1, 4);
                    w.bits(shift as u64, 5);
                    for &c in &coefs {
                        w.signed(c, precision);
                    }
                    let mut res = x.clone();
                    for i in order..x.len() {
                        let sum: i64 = (0..order).map(|j| coefs[j] * x[i - 1 - j]).sum();
                        res[i] = x[i] - (sum >> shift);
                    }
                    residual(w, &res, order, porder, false);
                }
            }
        }

        pub fn encode(input: &[Vec<i64>], bps: u32, rate: u32, plan: &[FramePlan], variable: bool) -> Vec<u8> {
            encode_inner(input, bps, rate, plan, variable, 0)
        }

        pub fn encode_with_wasted(input: &[Vec<i64>], bps: u32, rate: u32, plan: &[FramePlan], wasted: u32) -> Vec<u8> {
            encode_inner(input, bps, rate, plan, false, wasted)
        }

        fn encode_inner(input: &[Vec<i64>], bps: u32, rate: u32, plan: &[FramePlan], variable: bool, wasted: u32) -> Vec<u8> {
            let ch = input.len();
            let total: usize = input[0].len();
            let mut md5 = Md5::new();
            let bytes = bps.div_ceil(8) as usize;
            for i in 0..total {
                for c in input {
                    md5.update(&(c[i] as u32).to_le_bytes()[..bytes]);
                }
            }
            let mut out = b"fLaC".to_vec();
            let max_block = plan.iter().map(|p| p.block).max().unwrap_or(16) as u16;
            let mut si = vec![0u8; 34];
            si[0..2].copy_from_slice(&16u16.to_be_bytes());
            si[2..4].copy_from_slice(&max_block.max(16).to_be_bytes());
            let packed: u64 = ((rate as u64) << 44) | (((ch as u64) - 1) << 41) | (((bps as u64) - 1) << 36) | total as u64;
            si[10..18].copy_from_slice(&packed.to_be_bytes());
            si[18..34].copy_from_slice(&md5.finalize());
            out.push(0x00);
            out.extend_from_slice(&(34u32.to_be_bytes()[1..]));
            out.extend_from_slice(&si);
            let vc = vorbis_comment::build("sonora test encoder", &[("TITLE", "Test")]);
            out.push(0x84);
            out.extend_from_slice(&((vc.len() as u32).to_be_bytes()[1..]));
            out.extend_from_slice(&vc);

            let mut pos = 0usize;
            for (fi, p) in plan.iter().enumerate() {
                let bs = p.block;
                let chans: Vec<Vec<i64>> = input.iter().map(|c| c[pos..pos + bs].to_vec()).collect();
                let coded: Vec<Vec<i64>> = match p.assignment {
                    8 => vec![chans[0].clone(), chans[0].iter().zip(&chans[1]).map(|(l, r)| l - r).collect()],
                    9 => vec![chans[0].iter().zip(&chans[1]).map(|(l, r)| l - r).collect(), chans[1].clone()],
                    10 => vec![
                        chans[0].iter().zip(&chans[1]).map(|(l, r)| (l + r) >> 1).collect(),
                        chans[0].iter().zip(&chans[1]).map(|(l, r)| l - r).collect(),
                    ],
                    _ => chans,
                };
                let mut w = BitWriter::default();
                w.bits(0xFFF8 | variable as u64, 16);
                w.bits(7, 4); // block size: 16-bit field
                w.bits(0, 4); // sample rate from STREAMINFO
                let assignment = if p.assignment >= 8 { p.assignment as u64 } else { ch as u64 - 1 };
                w.bits(assignment, 4);
                w.bits(0, 3); // sample size from STREAMINFO
                w.bits(0, 1);
                let number = if variable { pos as u64 + (1u64 << 33) } else { fi as u64 };
                for b in encode_number(number) {
                    w.bits(b as u64, 8);
                }
                w.bits(bs as u64 - 1, 16);
                let crc = super::super::crc8(&w.bytes);
                w.bits(crc as u64, 8);
                for (c, x) in coded.iter().enumerate() {
                    let side = matches!((p.assignment, c), (8, 1) | (9, 0) | (10, 1));
                    let sub = p.subs[c.min(p.subs.len() - 1)];
                    // Never claim more wasted bits than the coded channel has.
                    let or_all = x.iter().fold(0u64, |a, &v| a | v as u64);
                    let tz = if or_all == 0 { 0 } else { or_all.trailing_zeros() };
                    subframe(&mut w, x, bps + side as u32, sub, wasted.min(tz));
                }
                w.align();
                let crc16 = super::super::crc16(&w.bytes);
                w.bits(crc16 as u64, 16);
                out.extend_from_slice(&w.bytes);
                pos += bs;
            }
            out
        }
    }
}
