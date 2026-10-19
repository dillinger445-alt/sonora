//! Native Ogg metadata parsing for Vorbis, Opus, FLAC-in-Ogg and Speex streams.
//!
//! Pages are validated with the Ogg CRC-32; damaged pages are skipped. Only the
//! header packets at the start and the last pages of the file are read, so the
//! cost is independent of the file's length.

use super::vorbis_comment;
use crate::decode::sniff::{id3v2_size, read_up_to};
use crate::error::AnalysisError;
use std::fs::File;
use std::io::{BufReader, Seek, SeekFrom};
use std::path::Path;

#[derive(Clone, Debug, Default)]
pub struct OggInfo {
    /// "vorbis", "opus", "flac", "speex" or "unknown".
    pub codec_id: String,
    /// Decode sample rate (Vorbis: stream rate; Opus: always 48 000).
    pub sample_rate: u32,
    /// Opus: the original input sample rate recorded by the encoder (informational).
    pub input_sample_rate: Option<u32>,
    pub channels: u16,
    /// FLAC-in-Ogg bits per sample.
    pub bits_per_sample: Option<u16>,
    pub nominal_bitrate_kbps: Option<f64>,
    pub min_bitrate_kbps: Option<f64>,
    pub max_bitrate_kbps: Option<f64>,
    /// Average bitrate computed from the file size and duration.
    pub avg_bitrate_kbps: Option<f64>,
    /// From the last granule position (minus Opus pre-skip).
    pub duration_secs: Option<f64>,
    pub vendor: Option<String>,
    pub tags: Vec<(String, String)>,
    /// More than one logical stream (chained or multiplexed).
    pub chained: bool,
}

const MAX_PAGE_BODY: usize = 255 * 255;
/// Bytes scanned when resynchronising after garbage or a damaged page.
const MAX_RESYNC_BYTES: u64 = 1 << 20;
/// Pages read while looking for the header packets.
const MAX_HEADER_PAGES: usize = 4096;
/// Header packets larger than this (huge embedded artwork) are not reassembled.
const MAX_PACKET_BYTES: usize = 16 << 20;
/// Size of the tail window searched for the final granule position.
const TAIL_WINDOWS: [u64; 2] = [64 << 10, 1 << 20];

const fn crc_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut r = (i as u32) << 24;
        let mut k = 0;
        while k < 8 {
            r = if r & 0x8000_0000 != 0 { (r << 1) ^ 0x04C1_1DB7 } else { r << 1 };
            k += 1;
        }
        t[i] = r;
        i += 1;
    }
    t
}

static CRC_TABLE: [u32; 256] = crc_table();

fn crc_update(mut crc: u32, data: &[u8]) -> u32 {
    for &b in data {
        crc = (crc << 8) ^ CRC_TABLE[((crc >> 24) as u8 ^ b) as usize];
    }
    crc
}

/// Ogg page CRC: computed over the whole page with the checksum field zeroed.
pub(crate) fn page_crc(header: &[u8], lacing: &[u8], body: &[u8]) -> u32 {
    let mut h = [0u8; 27];
    h.copy_from_slice(&header[..27]);
    h[22..26].fill(0);
    let c = crc_update(0, &h);
    let c = crc_update(c, lacing);
    crc_update(c, body)
}

#[derive(Clone, Debug)]
struct Page {
    header_type: u8,
    granule: i64,
    serial: u32,
    lacing: Vec<u8>,
    body: Vec<u8>,
}

impl Page {
    fn is_bos(&self) -> bool {
        self.header_type & 0x02 != 0
    }
    fn is_continued(&self) -> bool {
        self.header_type & 0x01 != 0
    }
}

/// Parses one page from an in-memory buffer at `at`; returns the page and its
/// total length, or `None` if the bytes there are not a valid page.
fn page_at(buf: &[u8], at: usize) -> Option<(Page, usize)> {
    let h = buf.get(at..at + 27)?;
    if &h[0..4] != b"OggS" || h[4] != 0 {
        return None;
    }
    let nseg = h[26] as usize;
    let lacing = buf.get(at + 27..at + 27 + nseg)?;
    let body_len: usize = lacing.iter().map(|&l| l as usize).sum();
    let body = buf.get(at + 27 + nseg..at + 27 + nseg + body_len)?;
    let stored = u32::from_le_bytes([h[22], h[23], h[24], h[25]]);
    if page_crc(h, lacing, body) != stored {
        return None;
    }
    let page = Page {
        header_type: h[5],
        granule: i64::from_le_bytes([h[6], h[7], h[8], h[9], h[10], h[11], h[12], h[13]]),
        serial: u32::from_le_bytes([h[14], h[15], h[16], h[17]]),
        lacing: lacing.to_vec(),
        body: body.to_vec(),
    };
    Some((page, 27 + nseg + body_len))
}

/// Sequential page reader over a file.
struct PageReader {
    r: BufReader<File>,
    pos: u64,
    file_len: u64,
}

impl PageReader {
    fn read_exact_at_pos(&mut self, buf: &mut [u8]) -> std::io::Result<bool> {
        let n = read_up_to(&mut self.r, buf)?;
        self.pos += n as u64;
        Ok(n == buf.len())
    }

    fn seek(&mut self, pos: u64) -> std::io::Result<()> {
        self.r.seek(SeekFrom::Start(pos))?;
        self.pos = pos;
        Ok(())
    }

    /// Finds the next "OggS" at or after the current position.
    fn resync(&mut self) -> std::io::Result<bool> {
        let start = self.pos;
        let mut window = [0u8; 4];
        let mut filled = 0usize;
        let mut byte = [0u8; 1];
        while self.pos < self.file_len && self.pos - start < MAX_RESYNC_BYTES {
            if read_up_to(&mut self.r, &mut byte)? == 0 {
                return Ok(false);
            }
            self.pos += 1;
            if filled < 4 {
                window[filled] = byte[0];
                filled += 1;
            } else {
                window = [window[1], window[2], window[3], byte[0]];
            }
            if filled == 4 && &window == b"OggS" {
                let at = self.pos - 4;
                self.seek(at)?;
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Reads the next valid page, skipping garbage and pages with a bad CRC.
    fn next_page(&mut self) -> std::io::Result<Option<Page>> {
        loop {
            if self.pos + 27 > self.file_len {
                return Ok(None);
            }
            let page_start = self.pos;
            let mut h = [0u8; 27];
            if !self.read_exact_at_pos(&mut h)? {
                return Ok(None);
            }
            if &h[0..4] != b"OggS" || h[4] != 0 {
                self.seek(page_start + 1)?;
                if !self.resync()? {
                    return Ok(None);
                }
                continue;
            }
            let nseg = h[26] as usize;
            let mut lacing = vec![0u8; nseg];
            if !self.read_exact_at_pos(&mut lacing)? {
                return Ok(None);
            }
            let body_len: usize = lacing.iter().map(|&l| l as usize).sum();
            debug_assert!(body_len <= MAX_PAGE_BODY);
            let mut body = vec![0u8; body_len];
            if !self.read_exact_at_pos(&mut body)? {
                return Ok(None);
            }
            let stored = u32::from_le_bytes([h[22], h[23], h[24], h[25]]);
            if page_crc(&h, &lacing, &body) != stored {
                // Damaged page: look for the next capture pattern after it starts.
                self.seek(page_start + 1)?;
                if !self.resync()? {
                    return Ok(None);
                }
                continue;
            }
            return Ok(Some(Page {
                header_type: h[5],
                granule: i64::from_le_bytes([h[6], h[7], h[8], h[9], h[10], h[11], h[12], h[13]]),
                serial: u32::from_le_bytes([h[14], h[15], h[16], h[17]]),
                lacing,
                body,
            }));
        }
    }
}

/// Reassembles packets of one logical stream from its pages.
#[derive(Default)]
struct Packetizer {
    partial: Vec<u8>,
    /// The beginning of the current packet was lost (or it was too large).
    skipping: bool,
    packets: Vec<Vec<u8>>,
}

impl Packetizer {
    fn push_page(&mut self, page: &Page) {
        if !page.is_continued() && !self.partial.is_empty() {
            // A packet was left unterminated; it cannot be completed.
            self.partial.clear();
        }
        if page.is_continued() && self.partial.is_empty() && self.packets.is_empty() {
            self.skipping = true;
        }
        let mut offset = 0usize;
        for &lace in &page.lacing {
            let end = offset + lace as usize;
            if !self.skipping {
                if self.partial.len() + lace as usize > MAX_PACKET_BYTES {
                    self.skipping = true;
                    self.partial.clear();
                } else {
                    self.partial.extend_from_slice(&page.body[offset..end]);
                }
            }
            offset = end;
            if lace < 255 {
                if self.skipping {
                    self.skipping = false;
                    // Keep packet numbering intact: an oversized header packet
                    // is recorded as empty rather than dropped.
                    self.packets.push(Vec::new());
                } else {
                    self.packets.push(std::mem::take(&mut self.partial));
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Codec {
    Vorbis,
    Opus,
    Flac,
    Speex,
    Other,
}

fn identify(packet: &[u8]) -> Codec {
    if packet.len() >= 7 && packet[0] == 1 && &packet[1..7] == b"vorbis" {
        Codec::Vorbis
    } else if packet.starts_with(b"OpusHead") {
        Codec::Opus
    } else if packet.len() >= 5 && packet[0] == 0x7F && &packet[1..5] == b"FLAC" {
        Codec::Flac
    } else if packet.starts_with(b"Speex   ") {
        Codec::Speex
    } else {
        Codec::Other
    }
}

fn le16(b: &[u8], at: usize) -> Option<u16> {
    let s = b.get(at..at + 2)?;
    Some(u16::from_le_bytes([s[0], s[1]]))
}

fn le32(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at + 4)?;
    Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn kbps(bps: i32) -> Option<f64> {
    if bps > 0 {
        Some(bps as f64 / 1000.0)
    } else {
        None
    }
}

/// FLAC STREAMINFO body (34 bytes): (sample rate, channels, bits per sample).
fn flac_streaminfo(si: &[u8]) -> Option<(u32, u16, u16)> {
    if si.len() < 18 {
        return None;
    }
    let rate = ((si[10] as u32) << 12) | ((si[11] as u32) << 4) | ((si[12] as u32) >> 4);
    let channels = ((si[12] >> 1) & 0x07) as u16 + 1;
    let bits = ((((si[12] & 1) as u16) << 4) | ((si[13] >> 4) as u16)) + 1;
    Some((rate, channels, bits))
}

struct StreamHeaders {
    info: OggInfo,
    /// Opus pre-skip in 48 kHz samples.
    pre_skip: u64,
    /// Number of header packets needed to reach the tags.
    needed_packets: usize,
}

fn parse_identification(codec: Codec, p: &[u8]) -> Result<StreamHeaders, AnalysisError> {
    let mut info = OggInfo::default();
    let mut pre_skip = 0u64;
    let mut needed = 2usize;
    match codec {
        Codec::Vorbis => {
            // [type 1]["vorbis"][version u32][channels u8][rate u32][max i32][nominal i32][min i32]...
            if p.len() < 30 {
                return Err(AnalysisError::Corrupt("truncated Vorbis identification header".into()));
            }
            info.codec_id = "vorbis".into();
            info.channels = p[11] as u16;
            info.sample_rate = le32(p, 12).unwrap_or(0);
            info.max_bitrate_kbps = kbps(le32(p, 16).unwrap_or(0) as i32);
            info.nominal_bitrate_kbps = kbps(le32(p, 20).unwrap_or(0) as i32);
            info.min_bitrate_kbps = kbps(le32(p, 24).unwrap_or(0) as i32);
        }
        Codec::Opus => {
            // ["OpusHead"][version u8][channels u8][pre-skip u16][input rate u32][gain i16][family u8]
            if p.len() < 19 {
                return Err(AnalysisError::Corrupt("truncated OpusHead".into()));
            }
            info.codec_id = "opus".into();
            info.channels = p[9] as u16;
            pre_skip = le16(p, 10).unwrap_or(0) as u64;
            info.input_sample_rate = le32(p, 12).filter(|r| *r > 0);
            info.sample_rate = 48_000;
        }
        Codec::Flac => {
            // [0x7F]["FLAC"][major u8][minor u8][header packets u16be]["fLaC"][block header 4][STREAMINFO 34]
            if p.len() < 13 + 4 + 18 || &p[9..13] != b"fLaC" {
                return Err(AnalysisError::Corrupt("truncated Ogg FLAC mapping header".into()));
            }
            info.codec_id = "flac".into();
            let header_packets = u16::from_be_bytes([p[7], p[8]]) as usize;
            needed = 1 + if header_packets == 0 { 8 } else { header_packets.min(64) };
            let (rate, ch, bits) = flac_streaminfo(&p[17..]).ok_or_else(|| AnalysisError::Corrupt("invalid FLAC STREAMINFO".into()))?;
            info.sample_rate = rate;
            info.channels = ch;
            info.bits_per_sample = Some(bits);
        }
        Codec::Speex => {
            // ["Speex   "][version 20][version id i32][header size i32][rate i32][mode i32][mode bitstream i32][channels i32][bitrate i32]...
            if p.len() < 56 {
                return Err(AnalysisError::Corrupt("truncated Speex header".into()));
            }
            info.codec_id = "speex".into();
            info.sample_rate = le32(p, 36).unwrap_or(0);
            info.channels = le32(p, 48).unwrap_or(0).min(255) as u16;
            info.nominal_bitrate_kbps = kbps(le32(p, 52).unwrap_or(0) as i32);
        }
        Codec::Other => {
            return Err(AnalysisError::Unsupported("the Ogg stream does not contain a supported audio codec".into()));
        }
    }
    Ok(StreamHeaders { info, pre_skip, needed_packets: needed })
}

fn apply_comment_packets(codec: Codec, packets: &[Vec<u8>], info: &mut OggInfo) {
    let comment = match codec {
        Codec::Vorbis => packets.get(1).filter(|p| p.len() > 7 && p[0] == 3 && &p[1..7] == b"vorbis").and_then(|p| vorbis_comment::parse(&p[7..])),
        Codec::Opus => packets.get(1).filter(|p| p.starts_with(b"OpusTags")).and_then(|p| vorbis_comment::parse(&p[8..])),
        Codec::Speex => packets.get(1).and_then(|p| vorbis_comment::parse(p)),
        Codec::Flac => packets
            .iter()
            .skip(1)
            .find(|p| p.len() > 4 && p[0] & 0x7F == 4)
            .and_then(|p| vorbis_comment::parse(&p[4..])),
        Codec::Other => None,
    };
    if let Some(c) = comment {
        info.vendor = c.vendor;
        for (k, v) in c.tags {
            if !info.tags.iter().any(|(existing, _)| *existing == k) {
                info.tags.push((k, v));
            }
        }
    }
}

/// Searches the end of the file for the last valid page of `serial`.
/// Returns (granule of the last page of `serial`, serial of the very last valid page).
fn last_granule(f: &mut File, file_len: u64, serial: u32) -> std::io::Result<(Option<i64>, Option<u32>)> {
    let mut last_serial = None;
    for &window in TAIL_WINDOWS.iter() {
        let len = window.min(file_len);
        let start = file_len - len;
        f.seek(SeekFrom::Start(start))?;
        let mut buf = vec![0u8; len as usize];
        let n = read_up_to(f, &mut buf)?;
        buf.truncate(n);
        if buf.len() < 27 {
            return Ok((None, last_serial));
        }
        let mut granule = None;
        let mut i = buf.len() - 27;
        loop {
            if &buf[i..i + 4] == b"OggS" {
                if let Some((page, _)) = page_at(&buf, i) {
                    if last_serial.is_none() {
                        last_serial = Some(page.serial);
                    }
                    if page.serial == serial && page.granule >= 0 {
                        granule = Some(page.granule);
                        break;
                    }
                }
            }
            if i == 0 {
                break;
            }
            i -= 1;
        }
        if granule.is_some() || len == file_len {
            return Ok((granule, last_serial));
        }
    }
    Ok((None, last_serial))
}

/// Parses `path` as an Ogg file.
pub fn parse_file(path: &Path) -> Result<OggInfo, AnalysisError> {
    let mut f = File::open(path)?;
    let file_len = f.metadata()?.len();
    // Some taggers prepend an ID3v2 tag to Ogg files; skip it.
    let mut head = [0u8; 10];
    let n = read_up_to(&mut f, &mut head)?;
    let start = id3v2_size(&head[..n]).filter(|s| *s < file_len).unwrap_or(0);
    let mut reader = PageReader { r: BufReader::with_capacity(1 << 16, f.try_clone()?), pos: 0, file_len };
    reader.seek(start)?;

    // All beginning-of-stream pages come first; each carries one identification packet.
    let mut bos: Vec<(u32, Codec, Vec<u8>)> = Vec::new();
    let mut pending: Vec<Page> = Vec::new();
    let mut pages_read = 0usize;
    while let Some(page) = reader.next_page()? {
        pages_read += 1;
        if page.is_bos() {
            let first_packet_len: usize = page.lacing.iter().take_while(|&&l| l == 255).map(|&l| l as usize).sum::<usize>()
                + page.lacing.iter().find(|&&l| l < 255).map(|&l| l as usize).unwrap_or(0);
            let packet = page.body[..first_packet_len.min(page.body.len())].to_vec();
            bos.push((page.serial, identify(&packet), packet));
            pending.push(page);
            if pages_read > MAX_HEADER_PAGES {
                break;
            }
            continue;
        }
        pending.push(page);
        break;
    }
    if bos.is_empty() {
        return Err(AnalysisError::Corrupt("no valid Ogg stream found".into()));
    }
    let (serial, codec, ident) = bos
        .iter()
        .find(|(_, c, _)| *c != Codec::Other)
        .cloned()
        .ok_or_else(|| AnalysisError::Unsupported("the Ogg file does not contain a supported audio stream".into()))?;
    let StreamHeaders { mut info, pre_skip, needed_packets } = parse_identification(codec, &ident)?;
    info.chained = bos.len() > 1;

    let mut pk = Packetizer::default();
    for page in pending.iter().filter(|p| p.serial == serial) {
        pk.push_page(page);
    }
    while pk.packets.len() < needed_packets && pages_read < MAX_HEADER_PAGES {
        match reader.next_page()? {
            Some(page) => {
                pages_read += 1;
                if page.serial == serial {
                    pk.push_page(&page);
                } else if page.is_bos() {
                    info.chained = true;
                }
            }
            None => break,
        }
    }
    apply_comment_packets(codec, &pk.packets, &mut info);

    let (granule, last_serial) = last_granule(&mut f, file_len, serial)?;
    if last_serial.map(|s| !bos.iter().any(|(b, _, _)| *b == s)).unwrap_or(false) {
        info.chained = true;
    }
    if let Some(g) = granule {
        let samples = if codec == Codec::Opus { (g as u64).saturating_sub(pre_skip) } else { g as u64 };
        if info.sample_rate > 0 && samples > 0 {
            info.duration_secs = Some(samples as f64 / info.sample_rate as f64);
        }
    }
    if let Some(d) = info.duration_secs.filter(|d| *d > 0.0) {
        info.avg_bitrate_kbps = Some(file_len.saturating_sub(start) as f64 * 8.0 / d / 1000.0);
    }
    if info.sample_rate == 0 || info.channels == 0 {
        return Err(AnalysisError::Corrupt("the Ogg stream header declares no audio".into()));
    }
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_matches_reference() {
        // CRC of the ASCII string "123456789" with the Ogg parameters
        // (poly 0x04C11DB7, init 0, no reflection, no xorout) is 0x89A1897F.
        assert_eq!(crc_update(0, b"123456789"), 0x89A1_897F);
    }

    /// Builds one Ogg page with a valid CRC.
    fn page(header_type: u8, granule: i64, serial: u32, seq: u32, packets: &[&[u8]]) -> Vec<u8> {
        let mut lacing = Vec::new();
        let mut body = Vec::new();
        for p in packets {
            let mut n = p.len();
            while n >= 255 {
                lacing.push(255);
                n -= 255;
            }
            lacing.push(n as u8);
            body.extend_from_slice(p);
        }
        let mut h = Vec::new();
        h.extend_from_slice(b"OggS");
        h.push(0);
        h.push(header_type);
        h.extend_from_slice(&granule.to_le_bytes());
        h.extend_from_slice(&serial.to_le_bytes());
        h.extend_from_slice(&seq.to_le_bytes());
        h.extend_from_slice(&[0, 0, 0, 0]);
        h.push(lacing.len() as u8);
        let crc = page_crc(&h, &lacing, &body);
        h[22..26].copy_from_slice(&crc.to_le_bytes());
        [h, lacing, body].concat()
    }

    #[test]
    fn synthetic_opus_stream() {
        let mut head = b"OpusHead".to_vec();
        head.push(1); // version
        head.push(2); // channels
        head.extend_from_slice(&312u16.to_le_bytes());
        head.extend_from_slice(&44_100u32.to_le_bytes());
        head.extend_from_slice(&0i16.to_le_bytes());
        head.push(0);
        let mut tags = b"OpusTags".to_vec();
        tags.extend_from_slice(&super::vorbis_comment::build("libopus 1.4", &[("TITLE", "Synthetic"), ("BPM", "126")]));
        let mut file = page(0x02, 0, 7, 0, &[&head]);
        file.extend(page(0x00, 0, 7, 1, &[&tags]));
        file.extend(page(0x00, 24_000, 7, 2, &[&[0xFC; 40]]));
        // A damaged page (bad CRC) must be skipped, not trusted.
        let mut bad = page(0x00, 999_999, 7, 3, &[&[0xFC; 40]]);
        bad[30] ^= 0xFF;
        file.extend(bad);
        file.extend(page(0x04, 48_000 + 312, 7, 4, &[&[0xFC; 40]]));
        let dir = std::env::temp_dir().join(format!("sonora-ogg-unit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("synthetic.opus");
        std::fs::write(&p, &file).unwrap();
        let o = parse_file(&p).unwrap();
        assert_eq!(o.codec_id, "opus");
        assert_eq!(o.sample_rate, 48_000);
        assert_eq!(o.input_sample_rate, Some(44_100));
        assert_eq!(o.channels, 2);
        assert!((o.duration_secs.unwrap() - 1.0).abs() < 1e-9);
        assert_eq!(o.vendor.as_deref(), Some("libopus 1.4"));
        assert!(o.tags.contains(&("title".to_string(), "Synthetic".to_string())));
        assert!(o.tags.contains(&("bpm".to_string(), "126".to_string())));
        assert!(!o.chained);
        // A leading ID3v2 tag is skipped.
        let mut id3 = vec![b'I', b'D', b'3', 3, 0, 0, 0, 0, 0, 10];
        id3.extend_from_slice(&[0u8; 10]);
        id3.extend_from_slice(&file);
        std::fs::write(&p, &id3).unwrap();
        assert_eq!(parse_file(&p).unwrap().input_sample_rate, Some(44_100));
        // A second logical stream starting later (chained file) is flagged.
        let mut chained = file.clone();
        chained.extend(page(0x02, 0, 9, 0, &[&head]));
        chained.extend(page(0x04, 960, 9, 1, &[&[0xFC; 40]]));
        std::fs::write(&p, &chained).unwrap();
        let o = parse_file(&p).unwrap();
        assert!(o.chained);
        assert!((o.duration_secs.unwrap() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn packetizer_joins_across_pages() {
        let mk = |ht: u8, lacing: Vec<u8>, body: Vec<u8>| Page { header_type: ht, granule: 0, serial: 1, lacing, body };
        let mut pk = Packetizer::default();
        // Packet A (300 bytes) spans two pages; packet B (10 bytes) follows.
        pk.push_page(&mk(0x02, vec![255], vec![1u8; 255]));
        pk.push_page(&mk(0x01, vec![45, 10], [vec![1u8; 45], vec![2u8; 10]].concat()));
        assert_eq!(pk.packets.len(), 2);
        assert_eq!(pk.packets[0].len(), 300);
        assert_eq!(pk.packets[1], vec![2u8; 10]);
    }
}
