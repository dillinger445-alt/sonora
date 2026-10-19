//! Content-based container identification ("magic bytes").

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Container {
    Wav,
    Aiff,
    Flac,
    Mp3,
    AacAdts,
    Mp4,
    Ogg,
    Matroska,
    WavPack,
    Ape,
    Caf,
    Asf,
    Unknown,
}

impl Container {
    pub fn display_name(self) -> &'static str {
        match self {
            Container::Wav => "WAV",
            Container::Aiff => "AIFF",
            Container::Flac => "FLAC",
            Container::Mp3 => "MPEG audio",
            Container::AacAdts => "AAC (ADTS)",
            Container::Mp4 => "MP4/M4A",
            Container::Ogg => "Ogg",
            Container::Matroska => "Matroska/WebM",
            Container::WavPack => "WavPack",
            Container::Ape => "Monkey's Audio",
            Container::Caf => "CAF",
            Container::Asf => "ASF/WMA",
            Container::Unknown => "unknown",
        }
    }

    /// Inverse of [`Container::id`] (unknown ids map to `Unknown`).
    pub fn from_id(id: &str) -> Container {
        match id {
            "wav" => Container::Wav,
            "aiff" => Container::Aiff,
            "flac" => Container::Flac,
            "mpeg" => Container::Mp3,
            "adts" => Container::AacAdts,
            "mp4" => Container::Mp4,
            "ogg" => Container::Ogg,
            "matroska" => Container::Matroska,
            "wavpack" => Container::WavPack,
            "ape" => Container::Ape,
            "caf" => Container::Caf,
            "asf" => Container::Asf,
            _ => Container::Unknown,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Container::Wav => "wav",
            Container::Aiff => "aiff",
            Container::Flac => "flac",
            Container::Mp3 => "mpeg",
            Container::AacAdts => "adts",
            Container::Mp4 => "mp4",
            Container::Ogg => "ogg",
            Container::Matroska => "matroska",
            Container::WavPack => "wavpack",
            Container::Ape => "ape",
            Container::Caf => "caf",
            Container::Asf => "asf",
            Container::Unknown => "unknown",
        }
    }
}

/// Size of an ID3v2 tag at the start of `head`, if present (including header and footer).
pub fn id3v2_size(head: &[u8]) -> Option<u64> {
    if head.len() < 10 || &head[0..3] != b"ID3" {
        return None;
    }
    let flags = head[5];
    let sz = head[6..10].iter().try_fold(0u64, |acc, &b| if b & 0x80 != 0 { None } else { Some((acc << 7) | b as u64) })?;
    let footer = if flags & 0x10 != 0 { 10 } else { 0 };
    Some(10 + sz + footer)
}

/// Checks whether `b` starts with a plausible MPEG-1/2/2.5 Layer I–III frame header.
pub fn is_mpeg_audio_header(b: &[u8]) -> bool {
    if b.len() < 4 || b[0] != 0xFF || (b[1] & 0xE0) != 0xE0 {
        return false;
    }
    let version = (b[1] >> 3) & 0x3;
    let layer = (b[1] >> 1) & 0x3;
    let bitrate = (b[2] >> 4) & 0xF;
    let srate = (b[2] >> 2) & 0x3;
    version != 1 && layer != 0 && bitrate != 0xF && bitrate != 0 && srate != 3
}

/// AAC ADTS sync word (12 bits set, layer == 0).
pub fn is_adts_header(b: &[u8]) -> bool {
    b.len() >= 7 && b[0] == 0xFF && (b[1] & 0xF6) == 0xF0
}

/// Identifies the container from the first bytes of the file. `head` should
/// contain at least 64 bytes when available.
pub fn sniff_bytes(head: &[u8]) -> Container {
    if head.len() >= 12 {
        let riff = &head[0..4];
        if (riff == b"RIFF" || riff == b"RF64" || riff == b"BW64") && &head[8..12] == b"WAVE" {
            return Container::Wav;
        }
        if riff == b"FORM" && (&head[8..12] == b"AIFF" || &head[8..12] == b"AIFC") {
            return Container::Aiff;
        }
        if &head[4..8] == b"ftyp" {
            return Container::Mp4;
        }
    }
    if head.len() >= 4 {
        match &head[0..4] {
            b"fLaC" => return Container::Flac,
            b"OggS" => return Container::Ogg,
            b"wvpk" => return Container::WavPack,
            b"MAC " => return Container::Ape,
            b"caff" => return Container::Caf,
            [0x1A, 0x45, 0xDF, 0xA3] => return Container::Matroska,
            [0x30, 0x26, 0xB2, 0x75] => return Container::Asf,
            _ => {}
        }
    }
    if is_mpeg_audio_header(head) {
        return Container::Mp3;
    }
    if is_adts_header(head) {
        return Container::AacAdts;
    }
    Container::Unknown
}

/// Identifies the container of a file, skipping a leading ID3v2 tag
/// (common on MP3, occasionally present on FLAC/AAC files).
pub fn sniff_file(path: &Path) -> std::io::Result<Container> {
    let mut f = File::open(path)?;
    let mut head = [0u8; 64];
    let n = read_up_to(&mut f, &mut head)?;
    let head = &head[..n];
    if let Some(skip) = id3v2_size(head) {
        f.seek(SeekFrom::Start(skip))?;
        // Some files have padding/garbage between the tag and the first frame.
        let mut buf = vec![0u8; 8192];
        let m = read_up_to(&mut f, &mut buf)?;
        let buf = &buf[..m];
        let direct = sniff_bytes(buf);
        if direct != Container::Unknown {
            return Ok(direct);
        }
        for i in 0..buf.len().saturating_sub(4) {
            if is_mpeg_audio_header(&buf[i..]) {
                // Confirm with a second header at the expected frame distance.
                return Ok(Container::Mp3);
            }
        }
        return Ok(Container::Unknown);
    }
    Ok(sniff_bytes(head))
}

pub(crate) fn read_up_to(r: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut total = 0;
    while total < buf.len() {
        match r.read(&mut buf[total..]) {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic_detection() {
        let mut wav = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
        wav.resize(64, 0);
        assert_eq!(sniff_bytes(&wav), Container::Wav);
        let mut aiff = b"FORM\0\0\0\0AIFCCOMM".to_vec();
        aiff.resize(64, 0);
        assert_eq!(sniff_bytes(&aiff), Container::Aiff);
        assert_eq!(sniff_bytes(b"fLaC\0\0\0\x22"), Container::Flac);
        assert_eq!(sniff_bytes(b"OggS\0\x02"), Container::Ogg);
        assert_eq!(sniff_bytes(b"\0\0\0\x20ftypM4A "), Container::Mp4);
        // MPEG-1 Layer III, 128 kbps, 44.1 kHz.
        assert_eq!(sniff_bytes(&[0xFF, 0xFB, 0x90, 0x64]), Container::Mp3);
        assert_eq!(sniff_bytes(&[0xFF, 0xF1, 0x50, 0x80, 0, 0, 0]), Container::AacAdts);
        assert_eq!(sniff_bytes(b"hello world, not audio"), Container::Unknown);
    }

    #[test]
    fn id3_size() {
        let tag = [b'I', b'D', b'3', 4, 0, 0, 0, 0, 0x02, 0x01];
        assert_eq!(id3v2_size(&tag), Some(10 + 257));
        assert_eq!(id3v2_size(b"RIFF"), None);
    }
}
