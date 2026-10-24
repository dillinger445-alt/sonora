//! MPEG audio frame header, Xing/Info/VBRI and LAME tag parsing.
//!
//! These headers tell us whether an MP3 is CBR/VBR/ABR, which encoder made it
//! and — for LAME — the low-pass filter it applied, which is a valuable clue
//! when comparing the declared encoding against the measured spectrum.

use crate::decode::sniff::{id3v2_size, read_up_to};
use std::fs::File;
use std::io::{Seek, SeekFrom};
use std::path::Path;

#[derive(Clone, Debug, PartialEq)]
pub struct MpegInfo {
    pub version: &'static str,
    pub layer: u8,
    pub sample_rate: u32,
    pub channels: u16,
    pub channel_mode: &'static str,
    pub first_bitrate_kbps: u32,
    pub vbr_header: Option<&'static str>,
    pub frames: Option<u32>,
    pub bytes: Option<u32>,
    pub encoder: Option<String>,
    pub lame_vbr_method: Option<u8>,
    pub lowpass_hz: Option<u32>,
    pub bitrate_mode: Option<&'static str>,
    pub mode_source: &'static str,
    pub avg_bitrate_kbps: Option<f64>,
    pub encoder_delay: Option<u16>,
    pub encoder_padding: Option<u16>,
    /// Samples per channel in one frame (1152 for MPEG-1 Layer III).
    pub samples_per_frame: u32,
}

impl MpegInfo {
    /// The length the Xing, Info or VBRI header declares (its frame count,
    /// less the encoder's delay and padding). `None` without such a header:
    /// a plain stream declares no length.
    pub fn declared_secs(&self) -> Option<f64> {
        let frames = self.frames.filter(|f| *f > 0)?;
        if self.sample_rate == 0 || self.samples_per_frame == 0 {
            return None;
        }
        let trim = self.encoder_delay.unwrap_or(0) as f64 + self.encoder_padding.unwrap_or(0) as f64;
        let samples = (frames as f64 * self.samples_per_frame as f64 - trim).max(0.0);
        Some(samples / self.sample_rate as f64)
    }
}

const BR_V1_L1: [u32; 15] = [0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448];
const BR_V1_L2: [u32; 15] = [0, 32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384];
const BR_V1_L3: [u32; 15] = [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320];
const BR_V2_L1: [u32; 15] = [0, 32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256];
const BR_V2_L23: [u32; 15] = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160];

#[derive(Clone, Copy, Debug)]
struct FrameHeader {
    version: u8, // 3 = MPEG-1, 2 = MPEG-2, 0 = MPEG-2.5
    layer: u8,   // 1..3
    crc: bool,
    bitrate_kbps: u32,
    sample_rate: u32,
    padding: bool,
    mode: u8,
}

impl FrameHeader {
    fn parse(b: &[u8]) -> Option<FrameHeader> {
        if b.len() < 4 || b[0] != 0xFF || (b[1] & 0xE0) != 0xE0 {
            return None;
        }
        let version = (b[1] >> 3) & 3;
        let layer_bits = (b[1] >> 1) & 3;
        if version == 1 || layer_bits == 0 {
            return None;
        }
        let layer = 4 - layer_bits;
        let br_idx = (b[2] >> 4) as usize;
        let sr_idx = ((b[2] >> 2) & 3) as usize;
        if br_idx == 0 || br_idx == 15 || sr_idx == 3 {
            return None;
        }
        let table = match (version, layer) {
            (3, 1) => &BR_V1_L1,
            (3, 2) => &BR_V1_L2,
            (3, 3) => &BR_V1_L3,
            (_, 1) => &BR_V2_L1,
            _ => &BR_V2_L23,
        };
        let base = [44100u32, 48000, 32000][sr_idx];
        let sample_rate = match version {
            3 => base,
            2 => base / 2,
            _ => base / 4,
        };
        Some(FrameHeader {
            version,
            layer,
            crc: b[1] & 1 == 0,
            bitrate_kbps: table[br_idx],
            sample_rate,
            padding: (b[2] >> 1) & 1 == 1,
            mode: b[3] >> 6,
        })
    }

    fn frame_len(&self) -> usize {
        let br = self.bitrate_kbps as usize * 1000;
        let sr = self.sample_rate as usize;
        let pad = self.padding as usize;
        match self.layer {
            1 => (12 * br / sr + pad) * 4,
            2 => 144 * br / sr + pad,
            _ => {
                if self.version == 3 {
                    144 * br / sr + pad
                } else {
                    72 * br / sr + pad
                }
            }
        }
    }

    fn samples(&self) -> u32 {
        match (self.layer, self.version) {
            (1, _) => 384,
            (2, _) => 1152,
            (3, 3) => 1152,
            _ => 576,
        }
    }

    fn side_info_len(&self) -> usize {
        let mono = self.mode == 3;
        match (self.version == 3, mono) {
            (true, false) => 32,
            (true, true) => 17,
            (false, false) => 17,
            (false, true) => 9,
        }
    }
}

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

/// Parses the MPEG stream of `path`. Returns `None` when no valid frame is found.
pub fn parse_file(path: &Path) -> Option<MpegInfo> {
    let mut f = File::open(path).ok()?;
    let mut head = [0u8; 10];
    let n = read_up_to(&mut f, &mut head).ok()?;
    let start = id3v2_size(&head[..n]).unwrap_or(0);
    f.seek(SeekFrom::Start(start)).ok()?;
    // Read enough to cover a few hundred frames.
    let mut buf = vec![0u8; 512 * 1024];
    let n = read_up_to(&mut f, &mut buf).ok()?;
    buf.truncate(n);
    parse_bytes(&buf)
}

/// Parses MPEG frames from `buf` (which should start at or before the first frame).
pub fn parse_bytes(buf: &[u8]) -> Option<MpegInfo> {
    // Locate the first frame confirmed by a following frame header.
    let mut first = None;
    let limit = buf.len().saturating_sub(4).min(64 * 1024);
    for i in 0..limit {
        if let Some(h) = FrameHeader::parse(&buf[i..]) {
            let next = i + h.frame_len();
            if next + 4 <= buf.len() && FrameHeader::parse(&buf[next..]).is_some() {
                first = Some((i, h));
                break;
            }
        }
    }
    let (pos, h) = first?;
    let version = match h.version {
        3 => "MPEG-1",
        2 => "MPEG-2",
        _ => "MPEG-2.5",
    };
    let channel_mode = ["stereo", "joint stereo", "dual channel", "mono"][h.mode as usize];
    let mut info = MpegInfo {
        version,
        layer: h.layer,
        sample_rate: h.sample_rate,
        channels: if h.mode == 3 { 1 } else { 2 },
        channel_mode,
        first_bitrate_kbps: h.bitrate_kbps,
        vbr_header: None,
        frames: None,
        bytes: None,
        encoder: None,
        lame_vbr_method: None,
        lowpass_hz: None,
        bitrate_mode: None,
        mode_source: "unknown",
        avg_bitrate_kbps: None,
        encoder_delay: None,
        encoder_padding: None,
        samples_per_frame: h.samples(),
    };

    let frame = &buf[pos..(pos + h.frame_len()).min(buf.len())];
    let xing_off = 4 + if h.crc { 2 } else { 0 } + h.side_info_len();
    let mut is_header_frame = false;
    if frame.len() >= xing_off + 8 && (&frame[xing_off..xing_off + 4] == b"Xing" || &frame[xing_off..xing_off + 4] == b"Info") {
        is_header_frame = true;
        let tag = if &frame[xing_off..xing_off + 4] == b"Xing" { "Xing" } else { "Info" };
        info.vbr_header = Some(tag);
        let flags = be32(&frame[xing_off + 4..]);
        let mut p = xing_off + 8;
        if flags & 1 != 0 && frame.len() >= p + 4 {
            info.frames = Some(be32(&frame[p..]));
            p += 4;
        }
        if flags & 2 != 0 && frame.len() >= p + 4 {
            info.bytes = Some(be32(&frame[p..]));
            p += 4;
        }
        if flags & 4 != 0 {
            p += 100;
        }
        if flags & 8 != 0 {
            p += 4;
        }
        // LAME extension.
        if frame.len() >= p + 24 {
            let enc = &frame[p..p + 9];
            if enc.iter().all(|c| c.is_ascii_graphic() || *c == b' ' || *c == 0) && enc[0].is_ascii_alphabetic() {
                let s = String::from_utf8_lossy(enc).trim_end_matches('\0').trim().to_string();
                if !s.is_empty() {
                    let is_lame_like = s.starts_with("LAME") || s.starts_with("Lavc") || s.starts_with("Lavf") || s.starts_with("GOGO");
                    info.encoder = Some(s);
                    if is_lame_like {
                        let method = frame[p + 9] & 0x0F;
                        info.lame_vbr_method = Some(method);
                        let lp = frame[p + 10] as u32 * 100;
                        if lp > 0 {
                            info.lowpass_hz = Some(lp);
                        }
                        let d = &frame[p + 21..p + 24];
                        info.encoder_delay = Some(((d[0] as u16) << 4) | (d[1] as u16 >> 4));
                        info.encoder_padding = Some((((d[1] & 0x0F) as u16) << 8) | d[2] as u16);
                        info.bitrate_mode = match method {
                            1 | 8 => Some("CBR"),
                            2 | 9 => Some("ABR"),
                            3..=6 => Some("VBR"),
                            _ => None,
                        };
                        if info.bitrate_mode.is_some() {
                            info.mode_source = "LAME tag";
                        }
                    }
                }
            }
        }
        if info.bitrate_mode.is_none() {
            info.bitrate_mode = Some(if tag == "Info" { "CBR" } else { "VBR" });
            info.mode_source = if tag == "Info" { "Info header" } else { "Xing header" };
        }
    } else if frame.len() >= 36 + 26 && &frame[36..40] == b"VBRI" {
        is_header_frame = true;
        info.vbr_header = Some("VBRI");
        info.bytes = Some(be32(&frame[46..]));
        info.frames = Some(be32(&frame[50..]));
        info.bitrate_mode = Some("VBR");
        info.mode_source = "VBRI header";
        info.encoder = Some("Fraunhofer".into());
    }

    if let (Some(frames), Some(bytes)) = (info.frames, info.bytes) {
        if frames > 0 {
            let secs = frames as f64 * h.samples() as f64 / h.sample_rate as f64;
            if secs > 0.0 {
                info.avg_bitrate_kbps = Some(bytes as f64 * 8.0 / secs / 1000.0);
            }
        }
    }

    // Scan subsequent frames to infer CBR/VBR when no header says so, and to
    // estimate an average bitrate.
    let mut p = pos + if is_header_frame { h.frame_len() } else { 0 };
    let mut rates = Vec::new();
    while p + 4 <= buf.len() && rates.len() < 400 {
        match FrameHeader::parse(&buf[p..]) {
            Some(fh) if fh.sample_rate == h.sample_rate && fh.layer == h.layer => {
                rates.push(fh.bitrate_kbps);
                p += fh.frame_len().max(1);
            }
            _ => break,
        }
    }
    if !rates.is_empty() {
        if info.bitrate_mode.is_none() && rates.len() >= 20 {
            let all_same = rates.iter().all(|r| *r == rates[0]);
            info.bitrate_mode = Some(if all_same { "CBR" } else { "VBR" });
            info.mode_source = "inferred from frame headers";
        }
        if info.avg_bitrate_kbps.is_none() {
            info.avg_bitrate_kbps = Some(rates.iter().map(|r| *r as f64).sum::<f64>() / rates.len() as f64);
        }
    }
    Some(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cbr_stream(frames: usize) -> Vec<u8> {
        // MPEG-1 Layer III, 128 kbps, 44.1 kHz, no padding, joint stereo.
        let hdr = [0xFF, 0xFB, 0x90, 0x44];
        let h = FrameHeader::parse(&hdr).unwrap();
        let len = h.frame_len();
        assert_eq!(len, 417);
        let mut v = Vec::new();
        for _ in 0..frames {
            let start = v.len();
            v.extend_from_slice(&hdr);
            v.resize(start + len, 0);
        }
        v
    }

    #[test]
    fn infers_cbr_from_frames() {
        let info = parse_bytes(&cbr_stream(60)).unwrap();
        assert_eq!(info.version, "MPEG-1");
        assert_eq!(info.layer, 3);
        assert_eq!(info.sample_rate, 44100);
        assert_eq!(info.first_bitrate_kbps, 128);
        assert_eq!(info.bitrate_mode, Some("CBR"));
        assert_eq!(info.mode_source, "inferred from frame headers");
        assert!((info.avg_bitrate_kbps.unwrap() - 128.0).abs() < 1e-9);
    }

    #[test]
    fn parses_lame_info_tag() {
        let mut s = cbr_stream(30);
        // Side info for MPEG-1 joint stereo is 32 bytes -> Xing at offset 36.
        let x = 36;
        s[x..x + 4].copy_from_slice(b"Info");
        s[x + 4..x + 8].copy_from_slice(&3u32.to_be_bytes()); // frames + bytes
        s[x + 8..x + 12].copy_from_slice(&29u32.to_be_bytes());
        s[x + 12..x + 16].copy_from_slice(&(29u32 * 417).to_be_bytes());
        let p = x + 16;
        s[p..p + 9].copy_from_slice(b"LAME3.100");
        s[p + 9] = 0x01; // CBR
        s[p + 10] = 170; // 17 kHz lowpass
        let info = parse_bytes(&s).unwrap();
        assert_eq!(info.vbr_header, Some("Info"));
        assert_eq!(info.encoder.as_deref(), Some("LAME3.100"));
        assert_eq!(info.bitrate_mode, Some("CBR"));
        assert_eq!(info.lowpass_hz, Some(17_000));
        assert_eq!(info.frames, Some(29));
    }

    #[test]
    fn rejects_non_mpeg() {
        assert!(parse_bytes(&[0u8; 4096]).is_none());
    }
}
