//! Vorbis comment blocks (used by FLAC, Ogg Vorbis, Ogg Opus and Ogg FLAC).
//!
//! Layout: `vendor_length:u32le, vendor:utf8, count:u32le,
//! (length:u32le, "KEY=value":utf8) * count`. Keys are case-insensitive and
//! are returned lower-cased.

#[derive(Clone, Debug, Default, PartialEq)]
pub struct VorbisComment {
    pub vendor: Option<String>,
    pub tags: Vec<(String, String)>,
}

fn u32le(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at + 4)?;
    Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// Parses a comment block that starts at the vendor length field. Malformed
/// trailing entries are ignored rather than failing the whole block.
pub fn parse(data: &[u8]) -> Option<VorbisComment> {
    let vendor_len = u32le(data, 0)? as usize;
    let vendor_bytes = data.get(4..4usize.checked_add(vendor_len)?)?;
    let vendor = String::from_utf8_lossy(vendor_bytes).trim().to_string();
    let mut pos = 4 + vendor_len;
    let count = u32le(data, pos)? as usize;
    pos += 4;
    let mut tags = Vec::new();
    // Each entry needs at least 4 bytes, which bounds a corrupt count.
    for _ in 0..count.min(data.len() / 4) {
        let len = match u32le(data, pos) {
            Some(l) => l as usize,
            None => break,
        };
        pos += 4;
        let entry = match pos.checked_add(len).and_then(|end| data.get(pos..end)) {
            Some(e) => e,
            None => break,
        };
        pos += len;
        let text = String::from_utf8_lossy(entry);
        if let Some((k, v)) = text.split_once('=') {
            let key = k.trim().to_ascii_lowercase();
            let value = v.trim().to_string();
            // Skip embedded pictures and empty values.
            if key.is_empty() || value.is_empty() || key == "metadata_block_picture" || key == "coverart" {
                continue;
            }
            tags.push((key, value));
        }
    }
    Some(VorbisComment { vendor: if vendor.is_empty() { None } else { Some(vendor) }, tags })
}

/// Builds a comment block (used by tests and fixture tooling).
pub fn build(vendor: &str, tags: &[(&str, &str)]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    v.extend_from_slice(vendor.as_bytes());
    v.extend_from_slice(&(tags.len() as u32).to_le_bytes());
    for (k, val) in tags {
        let entry = format!("{k}={val}");
        v.extend_from_slice(&(entry.len() as u32).to_le_bytes());
        v.extend_from_slice(entry.as_bytes());
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_lowercase() {
        let b = build("reference libFLAC 1.4.3", &[("TITLE", "Night Drive"), ("Artist", "Somebody"), ("BPM", "124")]);
        let c = parse(&b).unwrap();
        assert_eq!(c.vendor.as_deref(), Some("reference libFLAC 1.4.3"));
        assert_eq!(c.tags[0], ("title".to_string(), "Night Drive".to_string()));
        assert_eq!(c.tags[1].0, "artist");
        assert_eq!(c.tags[2], ("bpm".to_string(), "124".to_string()));
    }

    #[test]
    fn tolerates_corrupt_counts() {
        let mut b = build("v", &[("TITLE", "x")]);
        // Claim a huge number of entries.
        let n = b.len();
        b[5..9].copy_from_slice(&u32::MAX.to_le_bytes());
        let c = parse(&b[..n]).unwrap();
        assert_eq!(c.tags.len(), 1);
        assert!(parse(&[1, 0]).is_none());
    }
}
