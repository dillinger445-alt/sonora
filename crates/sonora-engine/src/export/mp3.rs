//! Re-tagging MP3 files without touching the audio: the MPEG frames are
//! copied byte for byte behind a new ID3v2 tag, and old tags (ID3v2 at the
//! start, ID3v1, APEv2 and Lyrics3 at the end) are left out.

use crate::decode::sniff::read_up_to;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

/// Byte range `[start, end)` of the MPEG audio in an MP3 file.
pub fn audio_range(path: &Path) -> std::io::Result<(u64, u64)> {
    let mut f = File::open(path)?;
    let len = f.metadata()?.len();
    // Leading ID3v2 tags (occasionally more than one).
    let mut start = 0u64;
    loop {
        f.seek(SeekFrom::Start(start))?;
        let mut h = [0u8; 10];
        if read_up_to(&mut f, &mut h)? < 10 || &h[..3] != b"ID3" {
            break;
        }
        let size = ((h[6] as u64 & 0x7F) << 21) | ((h[7] as u64 & 0x7F) << 14) | ((h[8] as u64 & 0x7F) << 7) | (h[9] as u64 & 0x7F);
        let footer = if h[5] & 0x10 != 0 { 10 } else { 0 };
        start += 10 + size + footer;
        if start >= len {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "the file holds a tag but no audio"));
        }
    }
    let mut end = len;
    let tail = |f: &mut File, at: u64, n: usize| -> std::io::Result<Vec<u8>> {
        let mut b = vec![0u8; n];
        if at < start || at + n as u64 > len {
            return Ok(Vec::new());
        }
        f.seek(SeekFrom::Start(at))?;
        let got = read_up_to(f, &mut b)?;
        b.truncate(got);
        Ok(b)
    };
    // ID3v1 (and the "enhanced" TAG+ block before it).
    if end >= 128 && tail(&mut f, end - 128, 3)? == b"TAG" {
        end -= 128;
        if end >= 227 && tail(&mut f, end - 227, 4)? == b"TAG+" {
            end -= 227;
        }
    }
    for _ in 0..3 {
        // APEv2 footer.
        if end >= 32 {
            let footer = tail(&mut f, end - 32, 32)?;
            if footer.len() == 32 && &footer[..8] == b"APETAGEX" {
                let size = u32::from_le_bytes([footer[12], footer[13], footer[14], footer[15]]) as u64;
                let flags = u32::from_le_bytes([footer[20], footer[21], footer[22], footer[23]]);
                let header = if flags & 0x8000_0000 != 0 { 32 } else { 0 };
                if size + header <= end - start {
                    end -= size + header;
                    continue;
                }
            }
        }
        // Lyrics3 v2: "LYRICS200" preceded by a six-digit size.
        if end >= 15 {
            let t = tail(&mut f, end - 15, 15)?;
            if t.len() == 15 && &t[6..] == b"LYRICS200" {
                if let Ok(size) = std::str::from_utf8(&t[..6]).unwrap_or("x").parse::<u64>() {
                    if size + 15 <= end - start {
                        end -= size + 15;
                        continue;
                    }
                }
            }
        }
        break;
    }
    Ok((start, end))
}

/// Writes `dest` as `tag` followed by the audio frames of `src`. Returns the size written.
pub fn retag(src: &Path, dest: &Path, tag: &[u8]) -> std::io::Result<u64> {
    let (start, end) = audio_range(src)?;
    let mut input = File::open(src)?;
    input.seek(SeekFrom::Start(start))?;
    let mut out = std::io::BufWriter::with_capacity(1 << 20, File::create(dest)?);
    out.write_all(tag)?;
    let copied = std::io::copy(&mut input.take(end - start), &mut out)?;
    out.flush()?;
    out.into_inner().map_err(|e| e.into_error())?.sync_all()?;
    Ok(tag.len() as u64 + copied)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_are_found_and_skipped() {
        let dir = std::env::temp_dir().join(format!("sonora-mp3tags-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let audio: Vec<u8> = (0..5000u32).map(|i| if i % 417 == 0 { 0xFF } else { (i % 251) as u8 }).collect();
        let old = crate::export::id3::build(&crate::export::id3::TagSet { title: Some("Old".into()), ..Default::default() });
        let mut file = old.clone();
        file.extend_from_slice(&audio);
        // APEv2 tag with header and footer.
        let ape_items = b"some ape items".to_vec();
        let ape_size = (ape_items.len() + 32) as u32;
        let mut ape = b"APETAGEX".to_vec();
        ape.extend_from_slice(&2000u32.to_le_bytes());
        ape.extend_from_slice(&ape_size.to_le_bytes());
        ape.extend_from_slice(&1u32.to_le_bytes());
        ape.extend_from_slice(&0xA000_0000u32.to_le_bytes());
        ape.extend_from_slice(&[0; 8]);
        let mut footer = ape.clone();
        footer[20..24].copy_from_slice(&0x8000_0000u32.to_le_bytes());
        file.extend_from_slice(&ape);
        file.extend_from_slice(&ape_items);
        file.extend_from_slice(&footer);
        let mut v1 = b"TAG".to_vec();
        v1.resize(128, b' ');
        file.extend_from_slice(&v1);
        let src = dir.join("in.mp3");
        std::fs::write(&src, &file).unwrap();
        assert_eq!(audio_range(&src).unwrap(), (old.len() as u64, (old.len() + audio.len()) as u64));
        let new = crate::export::id3::build(&crate::export::id3::TagSet { title: Some("New".into()), ..Default::default() });
        let dest = dir.join("out.mp3");
        let size = retag(&src, &dest, &new).unwrap();
        let out = std::fs::read(&dest).unwrap();
        assert_eq!(size as usize, out.len());
        assert_eq!(&out[..new.len()], &new[..]);
        assert_eq!(&out[new.len()..], &audio[..]);
    }
}
