//! Embedded artwork: read natively from ID3 tags (MP3, AIFF, WAV) and FLAC
//! picture blocks, and through FFmpeg for everything else (M4A, Ogg…).

use super::id3::Picture;
use crate::decode::sniff::{id3v2_size, read_up_to, Container};
use crate::decode::tools::{quiet_command, run_with_timeout};
use crate::decode::ToolPaths;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::Duration;

/// Artwork larger than this is not copied (some players choke on huge images).
pub const MAX_PICTURE_BYTES: usize = 2 << 20;

fn synchsafe(b: &[u8]) -> usize {
    ((b[0] as usize & 0x7F) << 21) | ((b[1] as usize & 0x7F) << 14) | ((b[2] as usize & 0x7F) << 7) | (b[3] as usize & 0x7F)
}

fn remove_unsync(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        out.push(data[i]);
        i += if data[i] == 0xFF && data.get(i + 1) == Some(&0) { 2 } else { 1 };
    }
    out
}

/// Length of a terminated string in `enc` (including the terminator).
fn terminated_len(enc: u8, b: &[u8]) -> Option<usize> {
    if enc == 1 || enc == 2 {
        b.as_chunks::<2>().0.iter().position(|c| *c == [0, 0]).map(|p| p * 2 + 2)
    } else {
        b.iter().position(|&x| x == 0).map(|p| p + 1)
    }
}

/// The front cover (or else the first picture) of an ID3v2 tag.
pub fn from_id3(tag: &[u8]) -> Option<Picture> {
    if tag.len() < 10 || &tag[..3] != b"ID3" {
        return None;
    }
    let major = tag[3];
    let flags = tag[5];
    let end = (10 + synchsafe(&tag[6..10])).min(tag.len());
    let mut body = tag[10..end].to_vec();
    if major <= 3 && flags & 0x80 != 0 {
        body = remove_unsync(&body);
    }
    let mut i = 0;
    if flags & 0x40 != 0 && body.len() >= 4 {
        i = if major == 4 { synchsafe(&body[..4]) } else { u32::from_be_bytes([body[0], body[1], body[2], body[3]]) as usize + 4 };
    }
    let (id_len, hdr) = if major == 2 { (3, 6) } else { (4, 10) };
    let mut found: Option<Picture> = None;
    while i + hdr <= body.len() && body[i] != 0 {
        let id = &body[i..i + id_len];
        let size = match major {
            2 => ((body[i + 3] as usize) << 16) | ((body[i + 4] as usize) << 8) | body[i + 5] as usize,
            3 => u32::from_be_bytes([body[i + 4], body[i + 5], body[i + 6], body[i + 7]]) as usize,
            _ => synchsafe(&body[i + 4..i + 8]),
        };
        let fflags = if major >= 3 { u16::from_be_bytes([body[i + 8], body[i + 9]]) } else { 0 };
        let start = i + hdr;
        let stop = start.saturating_add(size);
        if size == 0 || stop > body.len() {
            break;
        }
        let compressed = (major == 3 && fflags & 0x00C0 != 0) || (major == 4 && fflags & 0x000C != 0);
        if !compressed && (id == b"APIC" || id == b"PIC") {
            let mut d = body[start..stop].to_vec();
            if major == 4 && fflags & 0x0002 != 0 {
                d = remove_unsync(&d);
            }
            if let Some((kind, pic)) = parse_apic(&d, major == 2) {
                if kind == 3 {
                    return Some(pic);
                }
                found.get_or_insert(pic);
            }
        }
        i = stop;
    }
    found
}

fn parse_apic(d: &[u8], v22: bool) -> Option<(u8, Picture)> {
    let enc = *d.first()?;
    let mut p = 1;
    if v22 {
        p += 3; // image format ("JPG", "PNG")
    } else {
        p += d[p..].iter().position(|&b| b == 0)? + 1; // MIME type
    }
    let kind = *d.get(p)?;
    p += 1;
    p += terminated_len(enc, d.get(p..)?)?; // description
    let data = d.get(p..)?.to_vec();
    if data.len() > MAX_PICTURE_BYTES {
        return None;
    }
    Some((kind, Picture::from_bytes(data)?))
}

/// The front cover (or else the first picture) of a FLAC file.
pub fn from_flac(path: &Path) -> Option<Picture> {
    let mut f = File::open(path).ok()?;
    let mut head = [0u8; 10];
    read_up_to(&mut f, &mut head).ok()?;
    let mut pos = id3v2_size(&head).unwrap_or(0);
    f.seek(SeekFrom::Start(pos)).ok()?;
    let mut magic = [0u8; 4];
    f.read_exact(&mut magic).ok()?;
    if &magic != b"fLaC" {
        return None;
    }
    pos += 4;
    let mut found: Option<Picture> = None;
    for _ in 0..128 {
        let mut h = [0u8; 4];
        f.read_exact(&mut h).ok()?;
        let last = h[0] & 0x80 != 0;
        let kind = h[0] & 0x7F;
        let len = u32::from_be_bytes([0, h[1], h[2], h[3]]) as usize;
        pos += 4;
        if kind == 6 && len <= MAX_PICTURE_BYTES + 4096 {
            let mut b = vec![0u8; len];
            f.read_exact(&mut b).ok()?;
            if let Some((ptype, pic)) = parse_flac_picture(&b) {
                if ptype == 3 {
                    return Some(pic);
                }
                found.get_or_insert(pic);
            }
        }
        pos += len as u64;
        if last {
            break;
        }
        f.seek(SeekFrom::Start(pos)).ok()?;
    }
    found
}

fn parse_flac_picture(b: &[u8]) -> Option<(u32, Picture)> {
    let u = |at: usize| -> Option<u32> { Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?)) };
    let kind = u(0)?;
    let mime_len = u(4)? as usize;
    let mut p = 8 + mime_len;
    let desc_len = u(p)? as usize;
    p += 4 + desc_len + 16; // description, width, height, depth, colours
    let data_len = u(p)? as usize;
    p += 4;
    let data = b.get(p..p + data_len)?.to_vec();
    if data.len() > MAX_PICTURE_BYTES {
        return None;
    }
    Some((kind, Picture::from_bytes(data)?))
}

/// The ID3 chunk of an AIFF or WAV file.
fn chunk_id3(path: &Path, container: Container) -> Option<Picture> {
    let chunk = match container {
        Container::Wav => match crate::decode::wav::parse_header(path) {
            Ok(h) | Err(crate::decode::wav::OpenError::NotNative(h)) => h.id3_chunk,
            Err(_) => None,
        },
        Container::Aiff => match crate::decode::aiff::parse_header(path) {
            Ok(h) | Err(crate::decode::aiff::OpenError::NotNative(h)) => h.id3_chunk,
            Err(_) => None,
        },
        _ => None,
    };
    let (off, len) = chunk?;
    if len > (MAX_PICTURE_BYTES as u64) * 2 {
        return None;
    }
    let mut f = File::open(path).ok()?;
    f.seek(SeekFrom::Start(off)).ok()?;
    let mut b = vec![0u8; len as usize];
    let n = read_up_to(&mut f, &mut b).ok()?;
    from_id3(&b[..n])
}

fn leading_id3(path: &Path) -> Option<Picture> {
    let mut f = File::open(path).ok()?;
    let mut head = [0u8; 10];
    read_up_to(&mut f, &mut head).ok()?;
    let size = id3v2_size(&head)? as usize;
    if size > MAX_PICTURE_BYTES * 2 {
        return None;
    }
    f.seek(SeekFrom::Start(0)).ok()?;
    let mut b = vec![0u8; size];
    let n = read_up_to(&mut f, &mut b).ok()?;
    from_id3(&b[..n])
}

/// The artwork of any audio file SONORA reads, if it has some.
pub fn extract(path: &Path, container: Container, tools: &ToolPaths) -> Option<Picture> {
    let native = match container {
        Container::Mp3 => leading_id3(path),
        Container::Flac => from_flac(path).or_else(|| leading_id3(path)),
        Container::Wav | Container::Aiff => chunk_id3(path, container),
        _ => None,
    };
    if native.is_some() || matches!(container, Container::Mp3 | Container::Wav | Container::Aiff) {
        return native;
    }
    // M4A, Ogg and others: let FFmpeg copy the attached picture out.
    let ffmpeg = tools.ffmpeg.as_ref()?;
    let mut cmd = quiet_command(ffmpeg);
    cmd.args(["-nostdin", "-hide_banner", "-loglevel", "error", "-protocol_whitelist", "file,pipe", "-i"])
        .arg(format!("file:{}", path.display()))
        .args(["-map", "0:v:0", "-c:v", "copy", "-frames:v", "1", "-f", "image2pipe", "pipe:1"]);
    let out = run_with_timeout(cmd, Duration::from_secs(20)).ok()?;
    if !out.success || out.stdout.len() > MAX_PICTURE_BYTES {
        return None;
    }
    Picture::from_bytes(out.stdout)
}
