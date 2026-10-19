//! ID3v2.3 tag writer.
//!
//! Version 2.3 is the one every DJ program and player reads (2.4 is still
//! not universal). Text is written as ISO-8859-1 when it fits, otherwise as
//! UTF-16 with a byte-order mark, which 2.3 readers handle.

/// The tags written into exported files.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TagSet {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub genre: Option<String>,
    /// Year (four digits) or a date starting with one.
    pub year: Option<String>,
    pub bpm: Option<f64>,
    /// Key text in the notation chosen for the export ("Am", "8A", "1m").
    pub key: Option<String>,
    pub comment: Option<String>,
    pub picture: Option<Picture>,
}

/// Embedded artwork.
#[derive(Clone, Debug, PartialEq)]
pub struct Picture {
    /// "image/jpeg" or "image/png".
    pub mime: String,
    pub data: Vec<u8>,
}

impl Picture {
    /// Recognises JPEG and PNG data by their signatures.
    pub fn from_bytes(data: Vec<u8>) -> Option<Picture> {
        let mime = if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
            "image/jpeg"
        } else if data.starts_with(b"\x89PNG\r\n\x1a\n") {
            "image/png"
        } else {
            return None;
        };
        Some(Picture { mime: mime.into(), data })
    }
}

/// Formats a tempo for TBPM: whole numbers without decimals, otherwise up to two.
pub fn bpm_text(bpm: f64) -> String {
    let r = (bpm * 100.0).round() / 100.0;
    if (r - r.round()).abs() < 1e-9 {
        format!("{}", r.round() as i64)
    } else {
        let s = format!("{r:.2}");
        s.trim_end_matches('0').to_string()
    }
}

fn latin1(s: &str) -> Option<Vec<u8>> {
    s.chars().map(|c| u8::try_from(u32::from(c)).ok()).collect()
}

/// Encoding byte plus encoded text (no terminator).
fn encode(s: &str) -> (u8, Vec<u8>) {
    match latin1(s) {
        Some(b) => (0, b),
        None => {
            let mut v = vec![0xFF, 0xFE];
            for u in s.encode_utf16() {
                v.extend_from_slice(&u.to_le_bytes());
            }
            (1, v)
        }
    }
}

fn frame(out: &mut Vec<u8>, id: &[u8; 4], payload: &[u8]) {
    out.extend_from_slice(id);
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(&[0, 0]);
    out.extend_from_slice(payload);
}

fn text_frame(out: &mut Vec<u8>, id: &[u8; 4], value: &str) {
    let value = value.trim();
    if value.is_empty() {
        return;
    }
    let (enc, bytes) = encode(value);
    let mut payload = vec![enc];
    payload.extend_from_slice(&bytes);
    frame(out, id, &payload);
}

fn synchsafe(n: u32) -> [u8; 4] {
    [((n >> 21) & 0x7F) as u8, ((n >> 14) & 0x7F) as u8, ((n >> 7) & 0x7F) as u8, (n & 0x7F) as u8]
}

/// Builds a complete ID3v2.3 tag ("ID3" header, frames and some padding).
pub fn build(tags: &TagSet) -> Vec<u8> {
    let mut frames = Vec::new();
    if let Some(v) = &tags.title {
        text_frame(&mut frames, b"TIT2", v);
    }
    if let Some(v) = &tags.artist {
        text_frame(&mut frames, b"TPE1", v);
    }
    if let Some(v) = &tags.album {
        text_frame(&mut frames, b"TALB", v);
    }
    if let Some(v) = &tags.genre {
        text_frame(&mut frames, b"TCON", v);
    }
    if let Some(y) = tags.year.as_deref().map(str::trim).filter(|y| y.len() >= 4 && y[..4].chars().all(|c| c.is_ascii_digit())) {
        text_frame(&mut frames, b"TYER", &y[..4]);
    }
    if let Some(b) = tags.bpm.filter(|b| *b > 0.0) {
        text_frame(&mut frames, b"TBPM", &bpm_text(b));
    }
    if let Some(v) = &tags.key {
        text_frame(&mut frames, b"TKEY", v);
    }
    if let Some(c) = tags.comment.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
        // encoding, language, empty short description (terminated), text
        let (enc, text) = encode(c);
        let mut payload = vec![enc];
        payload.extend_from_slice(b"eng");
        payload.extend_from_slice(if enc == 1 { &[0xFF, 0xFE, 0, 0][..] } else { &[0][..] });
        payload.extend_from_slice(&text);
        frame(&mut frames, b"COMM", &payload);
    }
    if let Some(p) = &tags.picture {
        let mut payload = vec![0u8];
        payload.extend_from_slice(p.mime.as_bytes());
        payload.push(0);
        payload.push(3); // front cover
        payload.push(0); // empty description
        payload.extend_from_slice(&p.data);
        frame(&mut frames, b"APIC", &payload);
    }
    let padding = 512;
    let size = (frames.len() + padding) as u32;
    let mut tag = Vec::with_capacity(10 + size as usize);
    tag.extend_from_slice(b"ID3\x03\x00\x00");
    tag.extend_from_slice(&synchsafe(size));
    tag.extend_from_slice(&frames);
    tag.resize(10 + size as usize, 0);
    tag
}

/// RIFF INFO list (for WAV): title, artist, album, genre, comment, year.
pub fn riff_info(tags: &TagSet) -> Vec<u8> {
    let mut body = b"INFO".to_vec();
    let mut add = |id: &[u8; 4], v: Option<&str>| {
        let Some(v) = v.map(str::trim).filter(|v| !v.is_empty()) else { return };
        // UTF-8, as FFmpeg and most current tools write INFO text.
        let bytes = v.as_bytes();
        let len = bytes.len() + 1;
        body.extend_from_slice(id);
        body.extend_from_slice(&(len as u32).to_le_bytes());
        body.extend_from_slice(bytes);
        body.push(0);
        if len % 2 == 1 {
            body.push(0);
        }
    };
    add(b"INAM", tags.title.as_deref());
    add(b"IART", tags.artist.as_deref());
    add(b"IPRD", tags.album.as_deref());
    add(b"IGNR", tags.genre.as_deref());
    add(b"ICMT", tags.comment.as_deref());
    add(b"ICRD", tags.year.as_deref().filter(|y| y.len() >= 4).map(|y| &y[..4]));
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::id3::parse_tag;

    #[test]
    fn tags_round_trip_through_the_reader() {
        let tags = TagSet {
            title: Some("Nachtzug (Extended Mix)".into()),
            artist: Some("Ève & Łukasz".into()),
            album: Some("Sommer".into()),
            genre: Some("Melodic House & Techno".into()),
            year: Some("2024-05-01".into()),
            bpm: Some(122.0),
            key: Some("8A".into()),
            comment: Some("Exported by SONORA".into()),
            picture: Picture::from_bytes(vec![0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3]),
        };
        let tag = build(&tags);
        assert_eq!(&tag[..5], b"ID3\x03\x00");
        let pairs = parse_tag(&tag);
        let get = |k: &str| pairs.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str());
        assert_eq!(get("title"), Some("Nachtzug (Extended Mix)"));
        assert_eq!(get("artist"), Some("Ève & Łukasz"), "UTF-16 when Latin-1 cannot hold it");
        assert_eq!(get("genre"), Some("Melodic House & Techno"));
        assert_eq!(get("date"), Some("2024"));
        assert_eq!(get("bpm"), Some("122"));
        assert_eq!(get("key"), Some("8A"));
        assert_eq!(get("comment"), Some("Exported by SONORA"));
        assert_eq!(crate::export::picture::from_id3(&tag).map(|p| p.data), Some(vec![0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3]));
    }

    #[test]
    fn bpm_text_is_compact() {
        assert_eq!(bpm_text(128.0), "128");
        assert_eq!(bpm_text(127.999), "128");
        assert_eq!(bpm_text(123.45), "123.45");
        assert_eq!(bpm_text(87.5), "87.5");
    }

    #[test]
    fn pictures_are_recognised() {
        assert_eq!(Picture::from_bytes(b"\x89PNG\r\n\x1a\nrest".to_vec()).map(|p| p.mime), Some("image/png".into()));
        assert!(Picture::from_bytes(b"GIF89a".to_vec()).is_none());
    }
}
