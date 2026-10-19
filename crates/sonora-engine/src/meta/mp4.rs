//! Native MP4/M4A (ISO BMFF / QuickTime) metadata parsing: codec, AAC profile,
//! sample rate, channels, duration, bitrate and iTunes-style tags.
//!
//! Only box headers and the `moov` box are read; media payloads (`mdat`) are
//! never loaded. Every size field is bounds-checked against the data that is
//! actually present, so truncated or hostile files produce an error instead of
//! a panic or an oversized allocation.

use crate::decode::sniff::read_up_to;
use crate::error::AnalysisError;
use std::fs::File;
use std::io::{Seek, SeekFrom};
use std::path::Path;

#[derive(Clone, Debug, Default)]
pub struct Mp4Info {
    /// Major brand from `ftyp` (e.g. "M4A ", "mp42").
    pub brand: Option<String>,
    /// FFmpeg-style codec id: "aac", "alac", "mp3", "ac3", "eac3", "opus", "flac", "pcm_*", or "unknown".
    pub codec_id: String,
    /// "LC", "HE-AAC", "HE-AACv2", "Main", "LTP", "xHE-AAC"... when known.
    pub codec_profile: Option<String>,
    /// Output sample rate (after SBR doubling when signalled).
    pub sample_rate: u32,
    pub channels: u16,
    /// Bits per sample for lossless codecs (ALAC/FLAC/PCM).
    pub bits_per_sample: Option<u16>,
    pub duration_secs: Option<f64>,
    pub avg_bitrate_kbps: Option<f64>,
    pub max_bitrate_kbps: Option<f64>,
    /// Encoding tool (`©too`) when present.
    pub encoder: Option<String>,
    /// Tags with canonical lower-case keys (title, artist, album, album_artist,
    /// genre, date, bpm, comment, key, encoder...).
    pub tags: Vec<(String, String)>,
    pub has_video: bool,
    pub fragmented: bool,
}

/// `moov` boxes larger than this are not loaded (real ones are a few MB at most).
const MAX_MOOV_BYTES: u64 = 64 << 20;
/// Maximum nesting depth followed inside `moov`.
const MAX_DEPTH: usize = 12;
/// Tag values longer than this are truncated.
const MAX_TAG_CHARS: usize = 4096;
/// Upper bound on top-level boxes visited (fragmented files can have many).
const MAX_TOP_LEVEL_BOXES: usize = 1_000_000;

fn corrupt(msg: &str) -> AnalysisError {
    AnalysisError::Corrupt(msg.to_string())
}

fn be16(b: &[u8], at: usize) -> Option<u16> {
    let s = b.get(at..at + 2)?;
    Some(u16::from_be_bytes([s[0], s[1]]))
}

fn be24(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at + 3)?;
    Some(((s[0] as u32) << 16) | ((s[1] as u32) << 8) | s[2] as u32)
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at + 4)?;
    Some(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

fn be64(b: &[u8], at: usize) -> Option<u64> {
    let s = b.get(at..at + 8)?;
    Some(u64::from_be_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]))
}

fn fourcc(b: &[u8], at: usize) -> Option<[u8; 4]> {
    let s = b.get(at..at + 4)?;
    Some([s[0], s[1], s[2], s[3]])
}

fn is_fourcc_like(k: &[u8; 4]) -> bool {
    k.iter().all(|&c| c == 0xA9 || (0x20..0x7F).contains(&c))
}

/// Iterates the child boxes of a box body held in memory. Iteration stops at
/// the first malformed header (size smaller than its header or past the end).
struct Boxes<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Boxes<'a> {
    fn new(data: &'a [u8]) -> Self {
        Boxes { data, pos: 0 }
    }
}

impl<'a> Iterator for Boxes<'a> {
    type Item = ([u8; 4], &'a [u8]);

    fn next(&mut self) -> Option<Self::Item> {
        let d = self.data;
        let start = self.pos;
        if start + 8 > d.len() {
            return None;
        }
        let size32 = be32(d, start)? as u64;
        let kind = fourcc(d, start + 4)?;
        let (size, hdr) = match size32 {
            0 => ((d.len() - start) as u64, 8usize),
            1 => (be64(d, start + 8)?, 16usize),
            s => (s, 8usize),
        };
        if size < hdr as u64 || size > (d.len() - start) as u64 {
            self.pos = d.len();
            return None;
        }
        let end = start + size as usize;
        self.pos = end;
        Some((kind, &d[start + hdr..end]))
    }
}

fn find_child<'a>(data: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    Boxes::new(data).find(|(k, _)| k == kind).map(|(_, b)| b)
}

/// MSB-first bit reader over a byte slice.
struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Bits { data, pos: 0 }
    }

    fn remaining(&self) -> usize {
        (self.data.len() * 8).saturating_sub(self.pos)
    }

    fn read(&mut self, n: u32) -> Option<u32> {
        if n == 0 {
            return Some(0);
        }
        if n > 32 || self.remaining() < n as usize {
            return None;
        }
        let mut v: u64 = 0;
        for _ in 0..n {
            let byte = self.data[self.pos / 8];
            let bit = (byte >> (7 - (self.pos % 8))) & 1;
            v = (v << 1) | bit as u64;
            self.pos += 1;
        }
        Some(v as u32)
    }

    fn byte_align(&mut self) {
        self.pos = self.pos.div_ceil(8) * 8;
    }
}

const AAC_RATES: [u32; 13] = [96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350];

/// Parsed MPEG-4 AudioSpecificConfig (ISO/IEC 14496-3 §1.6.2.1).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AacConfig {
    /// Audio object type of the core codec (2 = AAC-LC, 42 = USAC...).
    pub object_type: u32,
    /// Core (AAC) sampling rate.
    pub core_sample_rate: u32,
    /// channelConfiguration (0 = defined by a program config element).
    pub channel_config: u8,
    /// Decoded channel count (PCE count for configuration 0, 2 when PS is signalled).
    pub channels: u16,
    /// Spectral band replication signalled (explicitly or via sync extension).
    pub sbr: bool,
    /// Parametric stereo signalled.
    pub ps: bool,
    /// SBR output rate when signalled.
    pub extension_sample_rate: Option<u32>,
}

impl AacConfig {
    /// Sample rate a decoder produces.
    pub fn output_sample_rate(&self) -> u32 {
        if self.sbr {
            self.extension_sample_rate.unwrap_or(self.core_sample_rate * 2)
        } else {
            self.core_sample_rate
        }
    }

    /// Profile name in FFmpeg's vocabulary.
    pub fn profile(&self) -> String {
        if self.ps {
            return "HE-AACv2".into();
        }
        if self.sbr {
            return "HE-AAC".into();
        }
        match self.object_type {
            1 => "Main".into(),
            2 => "LC".into(),
            3 => "SSR".into(),
            4 => "LTP".into(),
            17 => "ER AAC LC".into(),
            19 => "ER AAC LTP".into(),
            23 => "LD".into(),
            39 => "ELD".into(),
            42 => "xHE-AAC".into(),
            n => format!("AOT {n}"),
        }
    }
}

fn read_aot(b: &mut Bits) -> Option<u32> {
    let aot = b.read(5)?;
    if aot == 31 {
        Some(32 + b.read(6)?)
    } else {
        Some(aot)
    }
}

fn read_rate(b: &mut Bits) -> Option<u32> {
    let idx = b.read(4)?;
    if idx == 15 {
        b.read(24).filter(|r| *r > 0)
    } else {
        AAC_RATES.get(idx as usize).copied()
    }
}

fn channels_for_config(cfg: u8) -> u16 {
    match cfg {
        1..=6 => cfg as u16,
        7 => 8,
        11 => 7,
        12 | 14 => 8,
        13 => 24,
        _ => 0,
    }
}

/// program_config_element(): returns the channel count it describes.
fn parse_pce(b: &mut Bits) -> Option<u16> {
    b.read(4)?; // element_instance_tag
    b.read(2)?; // object_type
    b.read(4)?; // sampling_frequency_index
    let front = b.read(4)?;
    let side = b.read(4)?;
    let back = b.read(4)?;
    let lfe = b.read(2)?;
    let assoc = b.read(3)?;
    let cc = b.read(4)?;
    if b.read(1)? == 1 {
        b.read(4)?; // mono_mixdown_element_number
    }
    if b.read(1)? == 1 {
        b.read(4)?; // stereo_mixdown_element_number
    }
    if b.read(1)? == 1 {
        b.read(3)?; // matrix_mixdown_idx + pseudo_surround_enable
    }
    let mut channels = 0u16;
    for _ in 0..(front + side + back) {
        let is_cpe = b.read(1)?;
        b.read(4)?;
        channels += if is_cpe == 1 { 2 } else { 1 };
    }
    for _ in 0..lfe {
        b.read(4)?;
        channels += 1;
    }
    for _ in 0..assoc {
        b.read(4)?;
    }
    for _ in 0..cc {
        b.read(5)?;
    }
    b.byte_align();
    let comment = b.read(8)?;
    for _ in 0..comment {
        b.read(8)?;
    }
    Some(channels)
}

/// GASpecificConfig(); updates `channels` when a PCE is present.
fn parse_ga_specific(b: &mut Bits, aot: u32, channel_config: u8, channels: &mut u16) -> Option<()> {
    b.read(1)?; // frameLengthFlag
    if b.read(1)? == 1 {
        b.read(14)?; // coreCoderDelay
    }
    let extension_flag = b.read(1)?;
    if channel_config == 0 {
        *channels = parse_pce(b)?;
    }
    if aot == 6 || aot == 20 {
        b.read(3)?; // layerNr
    }
    if extension_flag == 1 {
        if aot == 22 {
            b.read(5)?; // numOfSubFrame
            b.read(11)?; // layer_length
        }
        if matches!(aot, 17 | 19 | 20 | 23) {
            b.read(3)?; // resilience flags
        }
        b.read(1)?; // extensionFlag3
    }
    Some(())
}

/// Parses an AudioSpecificConfig. Returns `None` only when the mandatory
/// leading fields are missing or invalid; optional trailing data that cannot
/// be parsed simply leaves the SBR/PS flags at their explicit values.
pub fn parse_audio_specific_config(data: &[u8]) -> Option<AacConfig> {
    let mut b = Bits::new(data);
    let mut aot = read_aot(&mut b)?;
    let core_rate = read_rate(&mut b)?;
    let channel_config = b.read(4)? as u8;
    let mut cfg = AacConfig { core_sample_rate: core_rate, channel_config, channels: channels_for_config(channel_config), ..Default::default() };
    let mut explicit_extension = false;
    if aot == 5 || aot == 29 {
        explicit_extension = true;
        cfg.sbr = true;
        cfg.ps = aot == 29;
        cfg.extension_sample_rate = Some(read_rate(&mut b)?);
        aot = read_aot(&mut b)?;
        if aot == 22 {
            b.read(4)?; // extensionChannelConfiguration
        }
    }
    cfg.object_type = aot;
    if cfg.ps && cfg.channels == 1 {
        cfg.channels = 2;
    }
    let ga = matches!(aot, 1 | 2 | 3 | 4 | 6 | 7 | 17 | 19 | 20 | 21 | 22 | 23);
    let parsed_tail = if ga {
        let mut channels = cfg.channels;
        let ok = parse_ga_specific(&mut b, aot, channel_config, &mut channels).is_some();
        if channel_config == 0 && ok {
            cfg.channels = if cfg.ps && channels == 1 { 2 } else { channels };
        }
        ok
    } else {
        false
    };
    if !parsed_tail {
        return Some(cfg);
    }
    if matches!(aot, 17 | 19 | 20 | 21 | 22 | 23 | 24 | 25 | 26 | 27 | 39) {
        let ep_config = b.read(2);
        if !matches!(ep_config, Some(0) | Some(1)) {
            return Some(cfg); // ErrorProtectionSpecificConfig not supported
        }
    }
    // Backward-compatible (implicit) SBR/PS signalling.
    if !explicit_extension && b.remaining() >= 16 && b.read(11) == Some(0x2B7) {
        let ext_aot = match read_aot(&mut b) {
            Some(a) => a,
            None => return Some(cfg),
        };
        if ext_aot == 5 && b.read(1) == Some(1) {
            if let Some(rate) = read_rate(&mut b) {
                cfg.sbr = true;
                cfg.extension_sample_rate = Some(rate);
                if b.remaining() >= 12 && b.read(11) == Some(0x548) && b.read(1) == Some(1) {
                    cfg.ps = true;
                    if cfg.channels == 1 {
                        cfg.channels = 2;
                    }
                }
            }
        }
    }
    Some(cfg)
}

/// Reads an MPEG-4 descriptor header: returns (tag, payload length, header length).
fn descriptor(d: &[u8], at: usize) -> Option<(u8, usize, usize)> {
    let tag = *d.get(at)?;
    let mut len = 0usize;
    let mut i = at + 1;
    for _ in 0..4 {
        let b = *d.get(i)?;
        i += 1;
        len = (len << 7) | (b & 0x7F) as usize;
        if b & 0x80 == 0 {
            break;
        }
    }
    Some((tag, len, i - at))
}

#[derive(Clone, Debug, Default)]
struct Esds {
    object_type: u8,
    max_bitrate: u32,
    avg_bitrate: u32,
    asc: Option<Vec<u8>>,
}

fn parse_decoder_config(d: &[u8], esds: &mut Esds) -> Option<()> {
    esds.object_type = *d.first()?;
    esds.max_bitrate = be32(d, 5)?;
    esds.avg_bitrate = be32(d, 9)?;
    let mut pos = 13;
    while pos < d.len() {
        let (tag, len, hdr) = descriptor(d, pos)?;
        let body = d.get(pos + hdr..(pos + hdr).checked_add(len)?.min(d.len()))?;
        if tag == 0x05 {
            esds.asc = Some(body.to_vec());
        }
        pos += hdr + len;
    }
    Some(())
}

/// `esds` FullBox body (version/flags first).
fn parse_esds(body: &[u8]) -> Option<Esds> {
    let d = body.get(4..)?;
    let mut esds = Esds::default();
    let (tag, len, hdr) = descriptor(d, 0)?;
    let payload = d.get(hdr..(hdr + len).min(d.len()))?;
    match tag {
        0x03 => {
            let flags = *payload.get(2)?;
            let mut pos = 3;
            if flags & 0x80 != 0 {
                pos += 2;
            }
            if flags & 0x40 != 0 {
                pos += 1 + *payload.get(pos)? as usize;
            }
            if flags & 0x20 != 0 {
                pos += 2;
            }
            while pos < payload.len() {
                let (t, l, h) = descriptor(payload, pos)?;
                if t == 0x04 {
                    let dc = payload.get(pos + h..(pos + h).checked_add(l)?.min(payload.len()))?;
                    parse_decoder_config(dc, &mut esds)?;
                    return Some(esds);
                }
                pos += h + l;
            }
            None
        }
        0x04 => {
            parse_decoder_config(payload, &mut esds)?;
            Some(esds)
        }
        _ => None,
    }
}

/// What the first audio sample description says.
#[derive(Clone, Debug, Default)]
struct AudioEntry {
    codec_id: String,
    profile: Option<String>,
    sample_rate: u32,
    channels: u16,
    bits: Option<u16>,
    avg_bitrate: Option<u32>,
    max_bitrate: Option<u32>,
}

/// Where an audio sample entry's child boxes begin. ISO entries have 28 bytes
/// of fields; QuickTime sound descriptions v1/v2 append 16/36 bytes. The
/// version field is ambiguous across the two specs, so the offset is chosen by
/// checking which candidate actually starts a well-formed box.
fn audio_children_offset(body: &[u8]) -> usize {
    let version = be16(body, 8).unwrap_or(0);
    let candidates: &[usize] = match version {
        1 => &[44, 28],
        2 => &[64, 28],
        _ => &[28, 44, 64],
    };
    for &off in candidates {
        if off == body.len() {
            return off;
        }
        if let (Some(size), Some(kind)) = (be32(body, off), fourcc(body, off + 4)) {
            if size >= 8 && off + size as usize <= body.len() && is_fourcc_like(&kind) {
                return off;
            }
        }
    }
    28.min(body.len())
}

fn pcm_codec(float: bool, signed: bool, bits: u16, little_endian: bool) -> String {
    if float {
        format!("pcm_f{bits}{}", if little_endian { "le" } else { "be" })
    } else if bits == 8 {
        if signed { "pcm_s8".into() } else { "pcm_u8".into() }
    } else {
        format!("pcm_{}{bits}{}", if signed { "s" } else { "u" }, if little_endian { "le" } else { "be" })
    }
}

/// FLAC STREAMINFO (34 bytes): (sample rate, channels, bits per sample).
fn flac_streaminfo(si: &[u8]) -> Option<(u32, u16, u16)> {
    if si.len() < 18 {
        return None;
    }
    let rate = ((si[10] as u32) << 12) | ((si[11] as u32) << 4) | ((si[12] as u32) >> 4);
    let channels = ((si[12] >> 1) & 0x07) as u16 + 1;
    let bits = ((((si[12] & 1) as u16) << 4) | ((si[13] >> 4) as u16)) + 1;
    Some((rate, channels, bits))
}

fn parse_audio_entry(kind: &[u8; 4], body: &[u8]) -> AudioEntry {
    let mut e = AudioEntry { codec_id: "unknown".into(), ..Default::default() };
    if body.len() < 28 {
        return e;
    }
    let version = be16(body, 8).unwrap_or(0);
    e.channels = be16(body, 16).unwrap_or(0);
    let sample_size = be16(body, 18).unwrap_or(0);
    e.sample_rate = be32(body, 24).unwrap_or(0) >> 16;
    let child_off = audio_children_offset(body);
    // QuickTime sound description v2 carries the real format in extra fields.
    let mut lpcm_flags: Option<u32> = None;
    let mut v2_bits: Option<u32> = None;
    if version == 2 && child_off == 64 {
        if let Some(rate_bits) = be64(body, 32) {
            let rate = f64::from_bits(rate_bits);
            if rate.is_finite() && rate > 0.0 && rate < 10_000_000.0 {
                e.sample_rate = rate.round() as u32;
            }
        }
        if let Some(ch) = be32(body, 40).filter(|c| *c > 0 && *c <= 64) {
            e.channels = ch as u16;
        }
        v2_bits = be32(body, 48).filter(|b| *b > 0 && *b <= 64);
        lpcm_flags = be32(body, 52);
    }
    let children = body.get(child_off..).unwrap_or(&[]);
    // QuickTime nests codec configuration inside a `wave` box.
    let wave = find_child(children, b"wave");
    let lookup = |k: &[u8; 4]| find_child(children, k).or_else(|| wave.and_then(|w| find_child(w, k)));
    if let Some(srat) = lookup(b"srat") {
        if let Some(r) = be32(srat, 4).filter(|r| *r > 0) {
            e.sample_rate = r;
        }
    }
    if let Some(btrt) = lookup(b"btrt") {
        e.max_bitrate = be32(btrt, 4).filter(|b| *b > 0);
        e.avg_bitrate = be32(btrt, 8).filter(|b| *b > 0);
    }
    let mut kind = *kind;
    if &kind == b"enca" {
        if let Some(frma) = lookup(b"sinf").and_then(|s| find_child(s, b"frma")).and_then(|f| fourcc(f, 0)) {
            kind = frma;
        }
    }
    match &kind {
        b"mp4a" => {
            e.codec_id = "aac".into();
            if let Some(esds) = lookup(b"esds").and_then(parse_esds) {
                if esds.avg_bitrate > 0 {
                    e.avg_bitrate = Some(esds.avg_bitrate);
                }
                if esds.max_bitrate > 0 {
                    e.max_bitrate = Some(esds.max_bitrate);
                }
                match esds.object_type {
                    0x40 => {
                        if let Some(cfg) = esds.asc.as_deref().and_then(parse_audio_specific_config) {
                            if cfg.object_type == 34 {
                                e.codec_id = "mp3".into();
                            } else {
                                e.profile = Some(cfg.profile());
                            }
                            let rate = cfg.output_sample_rate();
                            if rate > 0 {
                                e.sample_rate = rate;
                            }
                            if cfg.channels > 0 {
                                e.channels = cfg.channels;
                            }
                        }
                    }
                    0x66 => e.profile = Some("Main".into()),
                    0x67 => e.profile = Some("LC".into()),
                    0x68 => e.profile = Some("SSR".into()),
                    0x69 | 0x6B => e.codec_id = "mp3".into(),
                    0xA5 => e.codec_id = "ac3".into(),
                    0xA6 => e.codec_id = "eac3".into(),
                    0xA9 => e.codec_id = "dts".into(),
                    0xAD => e.codec_id = "opus".into(),
                    0xDD => e.codec_id = "vorbis".into(),
                    _ => {}
                }
            }
        }
        b"alac" => {
            e.codec_id = "alac".into();
            if let Some(cookie) = lookup(b"alac") {
                // FullBox (version/flags) + 24-byte ALACSpecificConfig; tolerate a bare cookie.
                let c = if cookie.len() >= 28 { &cookie[4..] } else { cookie };
                if c.len() >= 24 {
                    let bits = c[5] as u16;
                    let ch = c[9] as u16;
                    let rate = be32(c, 20).unwrap_or(0);
                    if bits > 0 {
                        e.bits = Some(bits);
                    }
                    if ch > 0 {
                        e.channels = ch;
                    }
                    if rate > 0 {
                        e.sample_rate = rate;
                    }
                }
            }
            if e.bits.is_none() && sample_size > 0 {
                e.bits = Some(sample_size);
            }
        }
        b"fLaC" => {
            e.codec_id = "flac".into();
            if let Some(dfla) = lookup(b"dfLa") {
                // FullBox, then metadata blocks; STREAMINFO comes first.
                if let Some(block) = dfla.get(4..) {
                    if block.len() >= 4 && block[0] & 0x7F == 0 {
                        if let Some((rate, ch, bits)) = flac_streaminfo(&block[4..]) {
                            e.sample_rate = rate;
                            e.channels = ch;
                            e.bits = Some(bits);
                        }
                    }
                }
            }
        }
        b"Opus" => {
            e.codec_id = "opus".into();
            e.sample_rate = 48_000;
            if let Some(dops) = lookup(b"dOps") {
                if let Some(&ch) = dops.get(1) {
                    if ch > 0 {
                        e.channels = ch as u16;
                    }
                }
            }
        }
        b"ac-3" => {
            e.codec_id = "ac3".into();
            if let Some(d) = lookup(b"dac3").and_then(|d| be24(d, 0)) {
                let fscod = (d >> 22) & 0x3;
                let acmod = (d >> 11) & 0x7;
                let lfe = (d >> 10) & 0x1;
                let rate_code = (d >> 5) & 0x1F;
                const CH: [u16; 8] = [2, 1, 2, 3, 3, 4, 4, 5];
                const KBPS: [u32; 19] = [32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384, 448, 512, 576, 640];
                if let Some(r) = [48_000u32, 44_100, 32_000].get(fscod as usize) {
                    e.sample_rate = *r;
                }
                e.channels = CH[acmod as usize] + lfe as u16;
                if e.avg_bitrate.is_none() {
                    e.avg_bitrate = KBPS.get(rate_code as usize).map(|k| k * 1000);
                }
            }
        }
        b"ec-3" => {
            e.codec_id = "eac3".into();
            if let Some(d) = lookup(b"dec3").and_then(|d| be16(d, 0)) {
                let kbps = (d >> 3) as u32;
                if kbps > 0 && e.avg_bitrate.is_none() {
                    e.avg_bitrate = Some(kbps * 1000);
                }
            }
        }
        b".mp3" | b"mp3 " => e.codec_id = "mp3".into(),
        b"sowt" => {
            let bits = if sample_size > 0 { sample_size } else { 16 };
            e.codec_id = pcm_codec(false, true, bits, true);
            e.bits = Some(bits);
        }
        b"twos" => {
            let bits = if sample_size > 0 { sample_size } else { 16 };
            e.codec_id = pcm_codec(false, true, bits, false);
            e.bits = Some(bits);
        }
        b"in24" | b"in32" | b"fl32" | b"fl64" => {
            let little = lookup(b"enda").and_then(|d| be16(d, 0)).map(|v| v != 0).unwrap_or(false);
            let (float, bits) = match &kind {
                b"in24" => (false, 24),
                b"in32" => (false, 32),
                b"fl32" => (true, 32),
                _ => (true, 64),
            };
            e.codec_id = pcm_codec(float, true, bits, little);
            e.bits = Some(bits);
        }
        b"raw " => {
            e.codec_id = "pcm_u8".into();
            e.bits = Some(8);
        }
        b"lpcm" => {
            let flags = lpcm_flags.unwrap_or(0x0C); // signed, packed
            let bits = v2_bits.map(|b| b as u16).unwrap_or(if sample_size > 0 { sample_size } else { 16 });
            let float = flags & 0x1 != 0;
            let big = flags & 0x2 != 0;
            let signed = flags & 0x4 != 0;
            e.codec_id = pcm_codec(float, signed || float, bits, !big);
            e.bits = Some(bits);
        }
        b"ipcm" | b"fpcm" => {
            let pcmc = lookup(b"pcmC");
            let little = pcmc.and_then(|p| p.get(4)).map(|f| f & 1 != 0).unwrap_or(false);
            let bits = pcmc.and_then(|p| p.get(5)).map(|b| *b as u16).filter(|b| *b > 0).unwrap_or(sample_size.max(16));
            e.codec_id = pcm_codec(&kind == b"fpcm", true, bits, little);
            e.bits = Some(bits);
        }
        b"ulaw" => e.codec_id = "pcm_mulaw".into(),
        b"alaw" => e.codec_id = "pcm_alaw".into(),
        b"samr" => e.codec_id = "amr_nb".into(),
        b"sawb" => e.codec_id = "amr_wb".into(),
        b"ima4" => e.codec_id = "adpcm_ima_qt".into(),
        _ => {}
    }
    e
}

#[derive(Clone, Debug, Default)]
struct Track {
    handler: [u8; 4],
    timescale: u32,
    duration: Option<u64>,
    tkhd_duration: Option<u64>,
    /// Sum of non-empty edit durations (movie timescale).
    edit_duration: Option<u64>,
    audio: Option<AudioEntry>,
    stsz_total: Option<u64>,
}

fn full_box_duration(body: &[u8], v0_at: usize, v1_at: usize) -> (Option<u32>, Option<u64>) {
    // Returns (timescale, duration) for mvhd/mdhd-style boxes.
    match body.first() {
        Some(1) => {
            let ts = be32(body, v1_at);
            let d = be64(body, v1_at + 4).filter(|d| *d != u64::MAX && *d > 0);
            (ts, d)
        }
        Some(_) => {
            let ts = be32(body, v0_at);
            let d = be32(body, v0_at + 4).filter(|d| *d != u32::MAX && *d > 0).map(|d| d as u64);
            (ts, d)
        }
        None => (None, None),
    }
}

fn parse_elst(body: &[u8]) -> Option<u64> {
    let version = *body.first()?;
    let count = be32(body, 4)? as usize;
    let entry = if version == 1 { 20 } else { 12 };
    let mut total = 0u64;
    let mut any = false;
    for i in 0..count.min(body.len().saturating_sub(8) / entry) {
        let at = 8 + i * entry;
        let (dur, media_time) = if version == 1 {
            (be64(body, at)?, be64(body, at + 8)? as i64)
        } else {
            (be32(body, at)? as u64, be32(body, at + 4)? as i32 as i64)
        };
        if media_time >= 0 {
            total = total.saturating_add(dur);
            any = true;
        }
    }
    if any && total > 0 {
        Some(total)
    } else {
        None
    }
}

fn parse_stsz(body: &[u8]) -> Option<u64> {
    let size = be32(body, 4)? as u64;
    let count = be32(body, 8)? as u64;
    if size != 0 {
        return Some(size.saturating_mul(count));
    }
    let n = (count as usize).min(body.len().saturating_sub(12) / 4);
    let mut total = 0u64;
    for i in 0..n {
        total += be32(body, 12 + i * 4)? as u64;
    }
    Some(total)
}

fn parse_stz2(body: &[u8]) -> Option<u64> {
    let field = *body.get(7)? as u32;
    let count = be32(body, 8)? as usize;
    let data = body.get(12..)?;
    let mut total = 0u64;
    match field {
        4 => {
            for i in 0..count.min(data.len() * 2) {
                let b = data[i / 2];
                total += if i % 2 == 0 { (b >> 4) as u64 } else { (b & 0x0F) as u64 };
            }
        }
        8 => total = data.iter().take(count).map(|&b| b as u64).sum(),
        16 => {
            for i in 0..count.min(data.len() / 2) {
                total += be16(data, i * 2)? as u64;
            }
        }
        _ => return None,
    }
    Some(total)
}

fn parse_trak(body: &[u8], depth: usize) -> Track {
    let mut t = Track::default();
    if depth > MAX_DEPTH {
        return t;
    }
    for (kind, b) in Boxes::new(body) {
        match &kind {
            b"tkhd" => {
                t.tkhd_duration = match b.first() {
                    Some(1) => be64(b, 28).filter(|d| *d != u64::MAX && *d > 0),
                    Some(_) => be32(b, 20).filter(|d| *d != u32::MAX && *d > 0).map(|d| d as u64),
                    None => None,
                }
            }
            b"edts" => {
                if let Some(elst) = find_child(b, b"elst") {
                    t.edit_duration = parse_elst(elst);
                }
            }
            b"mdia" => {
                for (k, mb) in Boxes::new(b) {
                    match &k {
                        b"mdhd" => {
                            let (ts, d) = full_box_duration(mb, 12, 20);
                            t.timescale = ts.unwrap_or(0);
                            t.duration = d;
                        }
                        b"hdlr" => {
                            if let Some(h) = fourcc(mb, 8) {
                                t.handler = h;
                            }
                        }
                        b"minf" => {
                            if let Some(stbl) = find_child(mb, b"stbl") {
                                for (sk, sb) in Boxes::new(stbl) {
                                    match &sk {
                                        b"stsd" => {
                                            if let Some(entries) = sb.get(8..) {
                                                if let Some((ek, eb)) = Boxes::new(entries).next() {
                                                    t.audio = Some(parse_audio_entry(&ek, eb));
                                                }
                                            }
                                        }
                                        b"stsz" => t.stsz_total = parse_stsz(sb),
                                        b"stz2" => t.stsz_total = parse_stz2(sb),
                                        _ => {}
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    t
}

const ID3V1_GENRES: [&str; 148] = [
    "Blues", "Classic Rock", "Country", "Dance", "Disco", "Funk", "Grunge", "Hip-Hop", "Jazz", "Metal", "New Age", "Oldies", "Other", "Pop",
    "R&B", "Rap", "Reggae", "Rock", "Techno", "Industrial", "Alternative", "Ska", "Death Metal", "Pranks", "Soundtrack", "Euro-Techno",
    "Ambient", "Trip-Hop", "Vocal", "Jazz+Funk", "Fusion", "Trance", "Classical", "Instrumental", "Acid", "House", "Game", "Sound Clip",
    "Gospel", "Noise", "Alternative Rock", "Bass", "Soul", "Punk", "Space", "Meditative", "Instrumental Pop", "Instrumental Rock", "Ethnic",
    "Gothic", "Darkwave", "Techno-Industrial", "Electronic", "Pop-Folk", "Eurodance", "Dream", "Southern Rock", "Comedy", "Cult", "Gangsta",
    "Top 40", "Christian Rap", "Pop/Funk", "Jungle", "Native American", "Cabaret", "New Wave", "Psychedelic", "Rave", "Showtunes", "Trailer",
    "Lo-Fi", "Tribal", "Acid Punk", "Acid Jazz", "Polka", "Retro", "Musical", "Rock & Roll", "Hard Rock", "Folk", "Folk-Rock", "National Folk",
    "Swing", "Fast Fusion", "Bebop", "Latin", "Revival", "Celtic", "Bluegrass", "Avantgarde", "Gothic Rock", "Progressive Rock",
    "Psychedelic Rock", "Symphonic Rock", "Slow Rock", "Big Band", "Chorus", "Easy Listening", "Acoustic", "Humour", "Speech", "Chanson",
    "Opera", "Chamber Music", "Sonata", "Symphony", "Booty Bass", "Primus", "Porn Groove", "Satire", "Slow Jam", "Club", "Tango", "Samba",
    "Folklore", "Ballad", "Power Ballad", "Rhythmic Soul", "Freestyle", "Duet", "Punk Rock", "Drum Solo", "A Cappella", "Euro-House",
    "Dance Hall", "Goa", "Drum & Bass", "Club-House", "Hardcore", "Terror", "Indie", "BritPop", "", "Polsk Punk", "Beat",
    "Christian Gangsta Rap", "Heavy Metal", "Black Metal", "Crossover", "Contemporary Christian", "Christian Rock", "Merengue", "Salsa",
    "Thrash Metal", "Anime", "JPop", "Synthpop",
];

/// Decodes the value of an ilst `data` box (type indicator + locale + payload).
fn data_value(data: &[u8]) -> Option<(u32, &[u8])> {
    let type_code = be32(data, 0)? & 0x00FF_FFFF;
    Some((type_code, data.get(8..)?))
}

fn text_of(type_code: u32, payload: &[u8]) -> Option<String> {
    let s = match type_code {
        2 => {
            let units: Vec<u16> = payload.as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes(*c)).collect();
            String::from_utf16_lossy(&units)
        }
        _ => String::from_utf8_lossy(payload).to_string(),
    };
    let s = s.trim_matches(|c: char| c == '\0' || c.is_whitespace()).to_string();
    if s.is_empty() {
        None
    } else {
        Some(s.chars().take(MAX_TAG_CHARS).collect())
    }
}

fn int_of(payload: &[u8]) -> Option<i64> {
    match payload.len() {
        1 => Some(payload[0] as i8 as i64),
        2 => Some(i16::from_be_bytes([payload[0], payload[1]]) as i64),
        3 => Some(be24(payload, 0)? as i64),
        4 => Some(be32(payload, 0)? as i32 as i64),
        8 => Some(be64(payload, 0)? as i64),
        _ => None,
    }
}

fn canonical_freeform(name: &str) -> String {
    let n = name.trim().to_ascii_lowercase();
    let n = n.strip_prefix("com.apple.quicktime.").unwrap_or(&n).to_string();
    match n.as_str() {
        "initialkey" | "initial key" | "key" => "key".into(),
        "bpm" | "tempo" | "tmpo" => "bpm".into(),
        "encoding_tool" | "encoder" | "software" => "encoder".into(),
        "albumartist" | "album artist" | "album_artist" => "album_artist".into(),
        "creationdate" | "year" | "date" => "date".into(),
        other => other.to_string(),
    }
}

fn ilst_key(kind: &[u8; 4]) -> Option<&'static str> {
    Some(match kind {
        b"\xA9nam" => "title",
        b"\xA9ART" => "artist",
        b"aART" => "album_artist",
        b"\xA9alb" => "album",
        b"\xA9gen" => "genre",
        b"\xA9day" => "date",
        b"\xA9cmt" => "comment",
        b"\xA9too" => "encoder",
        b"\xA9wrt" => "composer",
        b"\xA9grp" => "grouping",
        b"desc" => "description",
        b"cprt" => "copyright",
        _ => return None,
    })
}

fn push_tag(tags: &mut Vec<(String, String)>, key: &str, value: String) {
    if !tags.iter().any(|(k, _)| k == key) {
        tags.push((key.to_string(), value));
    }
}

/// Parses a `meta` box (iTunes `ilst` items, or QuickTime `keys` + `ilst`).
fn parse_meta(body: &[u8], tags: &mut Vec<(String, String)>) {
    // ISO `meta` is a FullBox; QuickTime's is not. Detect by looking for a box.
    let inner = if fourcc(body, 4).map(|k| &k == b"hdlr").unwrap_or(false) { body } else { body.get(4..).unwrap_or(&[]) };
    let mut keys: Vec<String> = Vec::new();
    if let Some(k) = find_child(inner, b"keys") {
        let count = be32(k, 4).unwrap_or(0) as usize;
        let mut pos = 8;
        for _ in 0..count.min(4096) {
            let size = match be32(k, pos) {
                Some(s) if s >= 8 => s as usize,
                _ => break,
            };
            let name = match k.get(pos + 8..pos + size) {
                Some(n) => String::from_utf8_lossy(n).to_string(),
                None => break,
            };
            keys.push(name);
            pos += size;
        }
    }
    let ilst = match find_child(inner, b"ilst") {
        Some(i) => i,
        None => return,
    };
    for (kind, item) in Boxes::new(ilst) {
        let data = find_child(item, b"data").and_then(data_value);
        if &kind == b"----" {
            let name = find_child(item, b"name").and_then(|n| n.get(4..)).map(|n| String::from_utf8_lossy(n).to_string());
            if let (Some(name), Some((tc, payload))) = (name, data) {
                if let Some(v) = text_of(tc, payload) {
                    push_tag(tags, &canonical_freeform(&name), v);
                }
            }
            continue;
        }
        let (tc, payload) = match data {
            Some(d) => d,
            None => continue,
        };
        if &kind == b"tmpo" {
            if let Some(bpm) = int_of(payload).filter(|b| *b > 0) {
                push_tag(tags, "bpm", bpm.to_string());
            }
            continue;
        }
        if &kind == b"gnre" {
            if let Some(idx) = int_of(payload).filter(|i| *i >= 1) {
                if let Some(g) = ID3V1_GENRES.get(idx as usize - 1).filter(|g| !g.is_empty()) {
                    push_tag(tags, "genre", g.to_string());
                }
            }
            continue;
        }
        if let Some(key) = ilst_key(&kind) {
            if let Some(v) = text_of(tc, payload) {
                push_tag(tags, key, v);
            }
            continue;
        }
        // QuickTime metadata: the item type is a 1-based index into `keys`.
        if !keys.is_empty() {
            let idx = u32::from_be_bytes(kind) as usize;
            if idx >= 1 && idx <= keys.len() {
                let key = canonical_freeform(&keys[idx - 1]);
                let value = if matches!(tc, 21 | 22) { int_of(payload).map(|i| i.to_string()) } else { text_of(tc, payload) };
                if let Some(v) = value {
                    push_tag(tags, &key, v);
                }
            }
        }
    }
}

/// Parses the `moov` box body.
fn parse_moov(moov: &[u8], info: &mut Mp4Info, mdat_bytes: u64) -> Result<(), AnalysisError> {
    let mut movie_timescale = 0u32;
    let mut movie_duration: Option<u64> = None;
    let mut fragment_duration: Option<u64> = None;
    let mut sound: Option<Track> = None;
    let mut tracks = 0usize;
    for (kind, body) in Boxes::new(moov) {
        match &kind {
            b"mvhd" => {
                let (ts, d) = full_box_duration(body, 12, 20);
                movie_timescale = ts.unwrap_or(0);
                movie_duration = d;
            }
            b"trak" => {
                tracks += 1;
                let t = parse_trak(body, 1);
                match &t.handler {
                    b"vide" => info.has_video = true,
                    b"soun" if sound.is_none() => sound = Some(t),
                    _ => {}
                }
            }
            b"mvex" => {
                info.fragmented = true;
                if let Some(mehd) = find_child(body, b"mehd") {
                    fragment_duration = match mehd.first() {
                        Some(1) => be64(mehd, 4),
                        Some(_) => be32(mehd, 4).map(|d| d as u64),
                        None => None,
                    }
                    .filter(|d| *d > 0);
                }
            }
            b"udta" => {
                if let Some(meta) = find_child(body, b"meta") {
                    parse_meta(meta, &mut info.tags);
                }
            }
            b"meta" => parse_meta(body, &mut info.tags),
            _ => {}
        }
    }
    let track = match sound {
        Some(t) => t,
        None if tracks == 0 => return Err(corrupt("the MP4 movie box contains no tracks")),
        None => return Err(AnalysisError::NoAudio),
    };
    let audio = track.audio.clone().unwrap_or_default();
    info.codec_id = if audio.codec_id.is_empty() { "unknown".into() } else { audio.codec_id.clone() };
    info.codec_profile = audio.profile.clone();
    info.sample_rate = audio.sample_rate;
    info.channels = audio.channels;
    info.bits_per_sample = audio.bits;

    let movie_secs = |d: u64| if movie_timescale > 0 { Some(d as f64 / movie_timescale as f64) } else { None };
    info.duration_secs = track
        .edit_duration
        .and_then(movie_secs)
        .or_else(|| track.duration.filter(|_| track.timescale > 0).map(|d| d as f64 / track.timescale as f64))
        .or_else(|| track.tkhd_duration.and_then(movie_secs))
        .or_else(|| fragment_duration.and_then(movie_secs))
        .or_else(|| movie_duration.and_then(movie_secs))
        .filter(|d| d.is_finite() && *d > 0.0);

    // ALAC cookies and some btrt boxes report the uncompressed rate, so for
    // lossless codecs the measured payload size is the better estimate.
    let lossless = info.codec_id == "alac" || info.codec_id == "flac" || info.codec_id.starts_with("pcm_");
    let declared = audio.avg_bitrate.filter(|_| !lossless).map(|b| b as f64 / 1000.0);
    let measured = info.duration_secs.and_then(|d| {
        let bytes = track.stsz_total.filter(|t| *t > 0).or(if info.has_video { None } else { Some(mdat_bytes).filter(|m| *m > 0) })?;
        Some(bytes as f64 * 8.0 / d / 1000.0)
    });
    info.avg_bitrate_kbps = declared.or(measured).filter(|b| b.is_finite() && *b > 0.0);
    info.max_bitrate_kbps = audio.max_bitrate.filter(|_| !lossless).map(|b| b as f64 / 1000.0);

    if let Some((_, enc)) = info.tags.iter().find(|(k, _)| k == "encoder") {
        info.encoder = Some(enc.clone());
    }
    Ok(())
}

/// Parses `path` as an ISO BMFF / QuickTime file.
pub fn parse_file(path: &Path) -> Result<Mp4Info, AnalysisError> {
    let mut f = File::open(path)?;
    let file_len = f.metadata()?.len();
    let mut info = Mp4Info::default();
    let mut moov: Option<Vec<u8>> = None;
    let mut mdat_bytes = 0u64;
    let mut pos = 0u64;
    let mut visited = 0usize;
    let mut moov_truncated = false;
    while pos + 8 <= file_len && visited < MAX_TOP_LEVEL_BOXES {
        visited += 1;
        f.seek(SeekFrom::Start(pos))?;
        let mut h = [0u8; 16];
        if read_up_to(&mut f, &mut h[..8])? < 8 {
            break;
        }
        let size32 = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) as u64;
        let kind = [h[4], h[5], h[6], h[7]];
        if !is_fourcc_like(&kind) {
            if visited == 1 {
                return Err(corrupt("not an MP4/QuickTime file"));
            }
            break;
        }
        let (size, hdr) = match size32 {
            0 => (file_len - pos, 8u64),
            1 => {
                if read_up_to(&mut f, &mut h[8..16])? < 8 {
                    break;
                }
                (u64::from_be_bytes([h[8], h[9], h[10], h[11], h[12], h[13], h[14], h[15]]), 16u64)
            }
            s => (s, 8u64),
        };
        if size < hdr {
            break;
        }
        let end = pos.saturating_add(size);
        let truncated = end > file_len;
        match &kind {
            b"ftyp" => {
                let mut brand = [0u8; 4];
                if read_up_to(&mut f, &mut brand)? == 4 && is_fourcc_like(&brand) {
                    info.brand = Some(String::from_utf8_lossy(&brand).to_string());
                }
            }
            b"moov" if moov.is_none() => {
                if truncated {
                    moov_truncated = true;
                    break;
                }
                let len = size - hdr;
                if len > MAX_MOOV_BYTES {
                    return Err(AnalysisError::Unsupported("the MP4 movie box is implausibly large".into()));
                }
                let mut body = vec![0u8; len as usize];
                if (read_up_to(&mut f, &mut body)? as u64) < len {
                    moov_truncated = true;
                    break;
                }
                moov = Some(body);
            }
            b"moof" => info.fragmented = true,
            b"mdat" => mdat_bytes += end.min(file_len) - (pos + hdr).min(file_len),
            _ => {}
        }
        if truncated {
            break;
        }
        pos = end;
    }
    let moov = match moov {
        Some(m) => m,
        None if moov_truncated => return Err(corrupt("the MP4 movie box is truncated (incomplete file?)")),
        None => return Err(corrupt("no MP4 movie box found (incomplete or damaged file?)")),
    };
    parse_moov(&moov, &mut info, mdat_bytes)?;
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_iterator_bounds() {
        // Second box claims to extend past the end; iteration stops cleanly.
        let mut d = Vec::new();
        d.extend_from_slice(&12u32.to_be_bytes());
        d.extend_from_slice(b"free");
        d.extend_from_slice(&[1, 2, 3, 4]);
        d.extend_from_slice(&100u32.to_be_bytes());
        d.extend_from_slice(b"moov");
        let kinds: Vec<[u8; 4]> = Boxes::new(&d).map(|(k, _)| k).collect();
        assert_eq!(kinds, vec![*b"free"]);
    }

    #[test]
    fn descriptor_lengths() {
        assert_eq!(descriptor(&[0x03, 0x80, 0x80, 0x80, 0x25], 0), Some((0x03, 0x25, 5)));
        assert_eq!(descriptor(&[0x04, 0x17], 0), Some((0x04, 0x17, 2)));
        assert_eq!(descriptor(&[0x05], 0), None);
    }
}
