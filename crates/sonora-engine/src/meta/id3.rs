//! Minimal ID3v2.2/2.3/2.4 text-frame reader used as a metadata fallback
//! when ffprobe is unavailable (e.g. tags in AIFF/WAV `ID3 ` chunks or MP3s).

fn synchsafe(b: &[u8]) -> u32 {
    ((b[0] as u32 & 0x7F) << 21) | ((b[1] as u32 & 0x7F) << 14) | ((b[2] as u32 & 0x7F) << 7) | (b[3] as u32 & 0x7F)
}

fn remove_unsync(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        out.push(data[i]);
        if data[i] == 0xFF && i + 1 < data.len() && data[i + 1] == 0x00 {
            i += 2;
        } else {
            i += 1;
        }
    }
    out
}

fn decode_text(enc: u8, bytes: &[u8]) -> String {
    let s = match enc {
        0 => bytes.iter().map(|&b| b as char).collect::<String>(),
        1 | 2 => {
            let (mut be, mut body) = (enc == 2, bytes);
            if body.len() >= 2 {
                if body[0] == 0xFF && body[1] == 0xFE {
                    be = false;
                    body = &body[2..];
                } else if body[0] == 0xFE && body[1] == 0xFF {
                    be = true;
                    body = &body[2..];
                }
            }
            let units: Vec<u16> = body.as_chunks::<2>().0.iter().map(|c| if be { u16::from_be_bytes(*c) } else { u16::from_le_bytes(*c) }).collect();
            String::from_utf16_lossy(&units)
        }
        _ => String::from_utf8_lossy(bytes).to_string(),
    };
    // Multiple values are NUL separated (v2.4); join them for display.
    s.split('\0').map(str::trim).filter(|p| !p.is_empty()).collect::<Vec<_>>().join(", ")
}

fn map_id(id: &str) -> Option<&'static str> {
    Some(match id {
        "TIT2" | "TT2" => "title",
        "TPE1" | "TP1" => "artist",
        "TALB" | "TAL" => "album",
        "TCON" | "TCO" => "genre",
        "TYER" | "TDRC" | "TYE" => "date",
        "TBPM" | "TBP" => "bpm",
        "TKEY" | "TKE" => "key",
        "TSSE" | "TSS" => "encoder_settings",
        "TENC" | "TEN" => "encoded_by",
        _ => return None,
    })
}

/// Parses an ID3v2 tag (starting with "ID3") and returns `(key, value)` pairs.
pub fn parse_tag(tag: &[u8]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if tag.len() < 10 || &tag[0..3] != b"ID3" {
        return out;
    }
    let major = tag[3];
    let flags = tag[5];
    let size = synchsafe(&tag[6..10]) as usize;
    let end = (10 + size).min(tag.len());
    let mut body: Vec<u8> = tag[10..end].to_vec();
    if major <= 3 && flags & 0x80 != 0 {
        body = remove_unsync(&body);
    }
    let mut i = 0usize;
    if flags & 0x40 != 0 && body.len() >= 4 {
        // Extended header.
        let ext = if major == 4 { synchsafe(&body[0..4]) as usize } else { u32::from_be_bytes([body[0], body[1], body[2], body[3]]) as usize + 4 };
        i = ext.min(body.len());
    }
    let (id_len, hdr_len) = if major == 2 { (3, 6) } else { (4, 10) };
    while i + hdr_len <= body.len() {
        let id_bytes = &body[i..i + id_len];
        if id_bytes[0] == 0 {
            break; // padding
        }
        let id = String::from_utf8_lossy(id_bytes).to_string();
        let fsize = match major {
            2 => ((body[i + 3] as usize) << 16) | ((body[i + 4] as usize) << 8) | body[i + 5] as usize,
            3 => u32::from_be_bytes([body[i + 4], body[i + 5], body[i + 6], body[i + 7]]) as usize,
            _ => synchsafe(&body[i + 4..i + 8]) as usize,
        };
        let fflags = if major >= 3 { u16::from_be_bytes([body[i + 8], body[i + 9]]) } else { 0 };
        let start = i + hdr_len;
        let stop = start.saturating_add(fsize);
        if stop > body.len() || fsize == 0 {
            break;
        }
        let mut data: Vec<u8> = body[start..stop].to_vec();
        if major == 4 && fflags & 0x0002 != 0 {
            data = remove_unsync(&data);
        }
        // Skip compressed/encrypted frames.
        let unsupported = (major == 3 && fflags & 0x00C0 != 0) || (major == 4 && fflags & 0x000C != 0);
        if !unsupported && !data.is_empty() {
            if let Some(key) = map_id(&id) {
                let text = decode_text(data[0], &data[1..]);
                if !text.is_empty() && !out.iter().any(|(k, _)| k == key) {
                    out.push((key.to_string(), text));
                }
            } else if (id == "COMM" || id == "COM") && data.len() > 4 {
                // encoding(1) language(3) description\0 text
                let enc = data[0];
                let rest = &data[4..];
                let sep = if enc == 1 || enc == 2 {
                    rest.as_chunks::<2>().0.iter().position(|c| *c == [0, 0]).map(|p| p * 2 + 2)
                } else {
                    rest.iter().position(|&b| b == 0).map(|p| p + 1)
                };
                if let Some(s) = sep {
                    let text = decode_text(enc, &rest[s.min(rest.len())..]);
                    if !text.is_empty() && !out.iter().any(|(k, _)| k == "comment") {
                        out.push(("comment".into(), text));
                    }
                }
            }
        }
        i = stop;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame_v3(id: &str, payload: &[u8]) -> Vec<u8> {
        let mut f = id.as_bytes().to_vec();
        f.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        f.extend_from_slice(&[0, 0]);
        f.extend_from_slice(payload);
        f
    }

    #[test]
    fn reads_v23_text_frames() {
        let mut frames = Vec::new();
        frames.extend(frame_v3("TIT2", b"\x00Night Drive"));
        frames.extend(frame_v3("TPE1", b"\x03Artiste \xC3\xA9"));
        let utf16: Vec<u8> = {
            let mut v = vec![1u8, 0xFF, 0xFE];
            for u in "Album".encode_utf16() {
                v.extend_from_slice(&u.to_le_bytes());
            }
            v
        };
        frames.extend(frame_v3("TALB", &utf16));
        frames.extend(frame_v3("TBPM", b"\x00124"));
        frames.extend(vec![0u8; 20]);
        let size = frames.len() as u32;
        let mut tag = b"ID3\x03\x00\x00".to_vec();
        tag.extend_from_slice(&[(size >> 21) as u8 & 0x7F, (size >> 14) as u8 & 0x7F, (size >> 7) as u8 & 0x7F, size as u8 & 0x7F]);
        tag.extend(frames);
        let tags = parse_tag(&tag);
        assert!(tags.contains(&("title".into(), "Night Drive".into())));
        assert!(tags.contains(&("artist".into(), "Artiste é".into())));
        assert!(tags.contains(&("album".into(), "Album".into())));
        assert!(tags.contains(&("bpm".into(), "124".into())));
    }
}
