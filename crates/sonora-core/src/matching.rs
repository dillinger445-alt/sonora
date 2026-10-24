//! Picking a Soulseek file for one track of a YouTube playlist: the search
//! words to try, in order; the rules a file must pass (format, quality,
//! size, who shares it, how well its name matches); and the order the
//! files that pass come in. Pure functions over search results, so the
//! job in `jobs.rs` and the tests share them.

use crate::settings::{PlaylistRules, LOSSY_FORMATS};
use crate::slskd::FoundFolder;
use crate::youtube::{norm, Parsed};

/// A track to look for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Want {
    pub artist: Option<String>,
    pub title: String,
    /// "Original Mix", "Carl Craig Remix".
    pub mix: Option<String>,
    pub catalogue: Option<String>,
    /// How long the video is.
    pub seconds: Option<i64>,
}

impl Want {
    pub fn from_parsed(p: &Parsed, seconds: Option<i64>) -> Want {
        Want { artist: p.artist.clone(), title: p.title.clone(), mix: p.mix.clone(), catalogue: p.catalogue.clone(), seconds }
    }
}

/// Words a name is matched by: letters and digits, lower case, accents
/// folded, two characters or more (one-letter words match everything).
pub fn tokens(s: &str) -> Vec<String> {
    norm(s).split(' ').filter(|w| w.chars().count() >= 2 || w.chars().all(|c| c.is_ascii_digit())).map(str::to_string).collect()
}

/// The search words: punctuation becomes spaces, apostrophes stay (as
/// most file names keep them) unless `plain`, which also joins hyphenated
/// words ("Re-Up" → "ReUp").
fn search_words(s: &str, plain: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_alphanumeric() {
            out.push(c);
        } else if c == '\'' || c == '’' {
            if !plain {
                out.push(c);
            }
        } else if (c == '-' || c == '‐') && plain {
            // joined
        } else {
            out.push(' ');
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A name without what is in brackets and without "feat. …": "Song (Edit)
/// [Label] feat. X" → "Song".
fn bare(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0i32;
    for c in s.chars() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = (depth - 1).max(0),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    let lower = out.to_lowercase();
    let cut = [" feat. ", " feat ", " ft. ", " ft ", " featuring ", " with "]
        .iter()
        .filter_map(|m| lower.find(m))
        .min()
        .unwrap_or(out.len());
    out[..cut].trim().to_string()
}

/// The first of several artists: "A & B", "A, B", "A x B", "A vs B" → "A".
fn first_artist(s: &str) -> String {
    let lower = s.to_lowercase();
    let cut = [" & ", ", ", " x ", " vs ", " vs. ", " and ", " + "].iter().filter_map(|m| lower.find(m)).min().unwrap_or(s.len());
    s[..cut].trim().to_string()
}

/// Accents folded, punctuation and apostrophes dropped, hyphens joined:
/// "Beyoncé – Don't Stop (Re-Up)" → "beyonce dont stop reup".
fn simple(s: &str) -> String {
    let joined: String = s.chars().filter(|c| !matches!(c, '\'' | '’' | '-' | '‐')).collect();
    norm(&joined.replace('&', " "))
}

/// The search texts to try for a track, in order, each with its number,
/// from the most results to the fewest words that still say which track:
///
/// 1. the title alone (without its mix name)
/// 2. the title and its mix name
/// 3. the artist and the title
/// 4. the artist, the title and its mix name
/// 5. the first artist and the bare title (no brackets, no "feat.")
/// 6. the artist alone: every file of theirs, which the rules then narrow
///    to those whose names hold the title's words
/// 7. the title's three longest words, for long titles
/// 8. the catalogue number
/// 9. the title, then 10. the artist and the title, with accents and
///    punctuation simplified
///
/// Repeats, empty texts and single words of fewer than four characters
/// (they match too much to find anything) are left out.
pub fn variations(w: &Want) -> Vec<(u8, String)> {
    let title_mix = match &w.mix {
        Some(m) => format!("{} {m}", w.title),
        None => w.title.clone(),
    };
    let artist = w.artist.clone().unwrap_or_default();
    let has_artist = !artist.trim().is_empty();
    let with_artist = |t: &str| if has_artist { format!("{artist} {t}") } else { String::new() };
    let long_words = {
        let words: Vec<String> = search_words(&bare(&w.title), false).split(' ').filter(|x| !x.is_empty()).map(str::to_string).collect();
        if words.len() > 3 {
            let mut by_length: Vec<(usize, &String)> = words.iter().enumerate().collect();
            by_length.sort_by(|a, b| b.1.chars().count().cmp(&a.1.chars().count()).then(a.0.cmp(&b.0)));
            let mut keep: Vec<(usize, &String)> = by_length.into_iter().take(3).collect();
            keep.sort_by_key(|x| x.0);
            keep.into_iter().map(|x| x.1.as_str()).collect::<Vec<_>>().join(" ")
        } else {
            String::new()
        }
    };
    let raw = [
        (1u8, search_words(&w.title, false)),
        (2, search_words(&title_mix, false)),
        (3, search_words(&with_artist(&w.title), false)),
        (4, search_words(&with_artist(&title_mix), false)),
        (5, if has_artist { search_words(&format!("{} {}", first_artist(&bare(&artist)), bare(&w.title)), false) } else { String::new() }),
        (6, search_words(&artist, false)),
        (7, long_words),
        (8, search_words(w.catalogue.as_deref().unwrap_or(""), false)),
        (9, simple(&w.title)),
        (10, if has_artist { simple(&format!("{artist} {}", w.title)) } else { String::new() }),
    ];
    let mut out: Vec<(u8, String)> = Vec::new();
    for (n, text) in raw {
        let text = text.trim().to_string();
        if text.is_empty() || text.chars().count() > 200 {
            continue;
        }
        if !text.contains(' ') && text.chars().count() < 4 {
            continue;
        }
        if out.iter().any(|(_, t)| t.eq_ignore_ascii_case(&text)) {
            continue;
        }
        out.push((n, text));
    }
    out
}

/// A file from the results, judged.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub username: String,
    pub filename: String,
    pub name: String,
    pub ext: String,
    pub size: i64,
    pub bit_depth: Option<i64>,
    pub sample_rate: Option<i64>,
    pub bit_rate: Option<i64>,
    pub length: Option<i64>,
    pub lossless: bool,
    pub free_slot: bool,
    pub queue: i64,
    pub speed: i64,
    /// Share of the title's words found in the file's name (0–1).
    pub overlap: f64,
    /// Why it is out, or `None` when it passes every rule.
    pub rejected: Option<String>,
    /// Lower comes first: format, quality, free slot, queue, speed, match.
    pub rank: [i64; 6],
}

impl Candidate {
    pub fn to_json(&self) -> sonora_json::Value {
        sonora_json::obj! {
            "username" => self.username.as_str(),
            "filename" => self.filename.as_str(),
            "name" => self.name.as_str(),
            "ext" => self.ext.as_str(),
            "size" => self.size,
            "bitDepth" => self.bit_depth,
            "sampleRate" => self.sample_rate,
            "bitRate" => self.bit_rate,
            "length" => self.length,
            "lossless" => self.lossless,
            "freeSlot" => self.free_slot,
            "queue" => self.queue,
            "speed" => self.speed,
            "overlap" => sonora_json::num(self.overlap, 2),
            "rejected" => self.rejected.clone(),
        }
    }

    pub fn from_json(v: &sonora_json::Value) -> Option<Candidate> {
        use sonora_json::Value;
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        let i = |k: &str| v.get(k).and_then(Value::as_i64);
        Some(Candidate {
            username: s("username")?,
            filename: s("filename")?,
            name: s("name").unwrap_or_default(),
            ext: s("ext").unwrap_or_default(),
            size: i("size").unwrap_or(0),
            bit_depth: i("bitDepth"),
            sample_rate: i("sampleRate"),
            bit_rate: i("bitRate"),
            length: i("length"),
            lossless: v.get("lossless").and_then(Value::as_bool).unwrap_or(false),
            free_slot: v.get("freeSlot").and_then(Value::as_bool).unwrap_or(false),
            queue: i("queue").unwrap_or(0),
            speed: i("speed").unwrap_or(0),
            overlap: v.get("overlap").and_then(Value::as_f64).unwrap_or(0.0),
            rejected: s("rejected"),
            rank: [0; 6],
        })
    }
}

/// The extension a format is known by in the rules ("aif" and "aifc" are AIFF).
pub fn canonical(ext: &str) -> &str {
    match ext {
        "aif" | "aifc" => "aiff",
        "wave" | "bwf" => "wav",
        "m4a" | "mp4" => "m4a",
        e => e,
    }
}

const VARIANT_WORDS: &[&str] = &["remix", "edit", "live", "radio", "instrumental", "bootleg", "mashup", "acapella", "karaoke", "rework", "refix", "flip"];

/// Why the file's format is out, or its rank among the accepted ones.
fn format_rank(ext: &str, bit_rate: Option<i64>, rules: &PlaylistRules) -> Result<i64, String> {
    let e = canonical(ext);
    if let Some(i) = rules.formats.iter().position(|f| f == e) {
        return Ok(i as i64);
    }
    if e == "m4a" && rules.formats.iter().any(|f| f == "alac") {
        // ALAC in .m4a cannot be told from AAC by name: taken, and checked on arrival.
        return Ok(rules.formats.len() as i64);
    }
    if let Some(i) = LOSSY_FORMATS.iter().position(|f| *f == e) {
        if !rules.lossy {
            return Err(format!("{}: lossy formats are off", e.to_uppercase()));
        }
        return match bit_rate {
            Some(b) if b < rules.min_bitrate => Err(format!("{} at {b} kbps is below the {} kbps minimum", e.to_uppercase(), rules.min_bitrate)),
            Some(_) => Ok(rules.formats.len() as i64 + 1 + i as i64),
            None => Err(format!("{}: its bit rate is not reported", e.to_uppercase())),
        };
    }
    Err(format!("{} is not an accepted format", if e.is_empty() { "a file without an extension".to_string() } else { e.to_uppercase() }))
}

/// Lower is better. Unknown resolution ranks after a known one that fits.
fn quality_rank(depth: Option<i64>, rate: Option<i64>, bit_rate: Option<i64>, lossless: bool, rules: &PlaylistRules) -> i64 {
    if !lossless {
        return -bit_rate.unwrap_or(0);
    }
    match rules.quality.as_str() {
        "cd" => match (depth, rate) {
            (Some(d), Some(r)) => (d - 16).abs() * 10 + (r - 44_100).abs() / 1_000,
            (Some(d), None) => (d - 16).abs() * 10 + 20,
            (None, Some(r)) => (r - 44_100).abs() / 1_000 + 20,
            (None, None) => 50,
        },
        "highest" => -(depth.unwrap_or(0) * rate.unwrap_or(0) / 1_000),
        _ => 0,
    }
}

/// How a file's words match the wanted track: the share of the title's
/// words in its name, whether the artist is in its name or folder.
fn match_words(name_words: &[String], dir_words: &[String], w: &Want) -> (f64, bool) {
    let title: Vec<String> = tokens(&w.title);
    let overlap = if title.is_empty() { 1.0 } else { title.iter().filter(|t| name_words.contains(t)).count() as f64 / title.len() as f64 };
    let artist_words: Vec<String> = w.artist.as_deref().map(tokens).unwrap_or_default().into_iter().filter(|t| !["the", "dj", "mc", "and", "feat", "ft"].contains(&t.as_str())).collect();
    let artist_in = artist_words.is_empty() || artist_words.iter().any(|a| name_words.contains(a) || dir_words.contains(a));
    (overlap, artist_in)
}

/// Megabytes as the rest of SONORA shows them (1 MB = 1,000,000 bytes).
const MB: i64 = 1_000_000;

fn mb(n: i64) -> String {
    format!("{:.1} MB", n as f64 / MB as f64)
}

/// Judges every file in the results for `w`: the ones that pass the rules
/// first, best first, then the rejected ones with their reasons.
pub fn rank(folders: &[FoundFolder], w: &Want, rules: &PlaylistRules) -> Vec<Candidate> {
    let wanted_words: Vec<String> = {
        let mut v = tokens(&w.title);
        v.extend(w.mix.as_deref().map(tokens).unwrap_or_default());
        v
    };
    let mut out = Vec::new();
    for f in folders {
        let dir_words = tokens(&f.directory);
        for x in &f.files {
            let name_words = tokens(crate::slskd::stem_of(&x.name));
            let (overlap, artist_in) = match_words(&name_words, &dir_words, w);
            let ext = canonical(&x.ext).to_string();
            let mut rejected: Option<String> = None;
            let mut fr = 0;
            match format_rank(&x.ext, x.bit_rate, rules) {
                Ok(r) => fr = r,
                Err(why) => rejected = Some(why),
            }
            if rejected.is_none() {
                if let (Some(max), Some(d)) = (rules.max_bit_depth, x.bit_depth) {
                    if d > max {
                        rejected = Some(format!("{d}-bit is above the {max}-bit limit"));
                    }
                }
            }
            if rejected.is_none() {
                if let (Some(max), Some(r)) = (rules.max_sample_rate, x.sample_rate) {
                    if r > max {
                        rejected = Some(format!("{} kHz is above the {} kHz limit", r as f64 / 1000.0, max as f64 / 1000.0));
                    }
                }
            }
            if rejected.is_none() && rules.size_check {
                let secs = x.length.or(w.seconds).filter(|s| *s > 0);
                if let Some(secs) = secs {
                    if x.lossless {
                        let uncompressed = secs * x.sample_rate.unwrap_or(44_100) * x.bit_depth.unwrap_or(16) * 2 / 8;
                        let floor = if matches!(ext.as_str(), "wav" | "aiff") { uncompressed * 8 / 10 } else { uncompressed * 3 / 10 };
                        if x.size < floor {
                            rejected = Some(format!("{} is far smaller than a lossless file of {} should be (about {} or more)", mb(x.size), format_secs(secs), mb(floor)));
                        }
                    } else if let Some(b) = x.bit_rate {
                        let expected = secs * b * 1000 / 8;
                        if x.size < expected / 2 {
                            rejected = Some(format!("{} is too small for {} at {b} kbps", mb(x.size), format_secs(secs)));
                        }
                    }
                }
            }
            if rejected.is_none() {
                if let Some(max) = rules.max_size_mb {
                    if x.size > max * MB {
                        rejected = Some(format!("{} is above the {max} MB limit", mb(x.size)));
                    }
                }
            }
            if rejected.is_none() && rules.free_slot && !f.free_slot {
                rejected = Some(format!("{} has no free upload slot", f.username));
            }
            if rejected.is_none() && !f.free_slot && f.queue > rules.max_queue {
                rejected = Some(format!("{}’s queue holds {} downloads (the limit is {})", f.username, f.queue, rules.max_queue));
            }
            if rejected.is_none() && rules.min_speed_kbps > 0 && f.speed < rules.min_speed_kbps * 1000 {
                rejected = Some(format!("{} reports {} kB/s, below the {} kB/s minimum", f.username, f.speed / 1000, rules.min_speed_kbps));
            }
            if rejected.is_none() && overlap < rules.min_overlap {
                rejected = Some(format!("the name shares only {}% of the title’s words", (overlap * 100.0).round() as i64));
            }
            if rejected.is_none() && rules.require_artist && !artist_in {
                rejected = Some(format!("{} is not in the file or folder name", w.artist.as_deref().unwrap_or("the artist")));
            }
            if rejected.is_none() && rules.reject_variants {
                if let Some(v) = name_words.iter().find(|nw| VARIANT_WORDS.contains(&nw.as_str()) && !wanted_words.contains(nw)) {
                    rejected = Some(format!("the name says “{v}”, which the title does not"));
                }
            }
            let length_close = match (x.length, w.seconds) {
                (Some(a), Some(b)) if a > 0 && b > 0 => (a - b).abs() <= (b / 20).max(5),
                _ => false,
            };
            let mix_match = match &w.mix {
                Some(m) => tokens(m).iter().all(|t| name_words.contains(t)),
                None => !name_words.iter().any(|nw| VARIANT_WORDS.contains(&nw.as_str())),
            };
            let score = overlap * 100.0 + if artist_in { 10.0 } else { 0.0 } + if length_close { 5.0 } else { 0.0 } + if mix_match { 5.0 } else { 0.0 };
            let rank = [fr, quality_rank(x.bit_depth, x.sample_rate, x.bit_rate, x.lossless, rules), if f.free_slot { 0 } else { 1 }, f.queue, -(f.speed / 50_000), -(score.round() as i64)];
            out.push(Candidate {
                username: f.username.clone(),
                filename: x.filename.clone(),
                name: x.name.clone(),
                ext,
                size: x.size,
                bit_depth: x.bit_depth,
                sample_rate: x.sample_rate,
                bit_rate: x.bit_rate,
                length: x.length,
                lossless: x.lossless,
                free_slot: f.free_slot,
                queue: f.queue,
                speed: f.speed,
                overlap,
                rejected,
                rank,
            });
        }
    }
    out.sort_by(|a, b| a.rejected.is_some().cmp(&b.rejected.is_some()).then(a.rank.cmp(&b.rank)).then(a.filename.cmp(&b.filename)));
    out
}

fn format_secs(s: i64) -> String {
    format!("{}:{:02}", s / 60, s % 60)
}

/// At least one file passes the rules.
pub fn sufficient(candidates: &[Candidate]) -> bool {
    candidates.iter().any(|c| c.rejected.is_none())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slskd::FoundFile;

    fn want() -> Want {
        Want { artist: Some("Kerri Chandler".into()), title: "Rain".into(), mix: Some("Original Mix".into()), catalogue: Some("KCT001".into()), seconds: Some(432) }
    }

    fn file(name: &str, ext: &str, size: i64, depth: Option<i64>, rate: Option<i64>, bit_rate: Option<i64>, length: Option<i64>) -> FoundFile {
        FoundFile {
            filename: format!("@@abc\\Music\\Kerri Chandler\\Album\\{name}.{ext}"),
            name: format!("{name}.{ext}"),
            ext: ext.into(),
            size,
            bit_depth: depth,
            sample_rate: rate,
            bit_rate,
            length,
            lossless: crate::slskd::LOSSLESS.contains(&ext),
        }
    }

    fn folder(username: &str, free_slot: bool, queue: i64, speed: i64, files: Vec<FoundFile>) -> FoundFolder {
        FoundFolder { username: username.into(), directory: "@@abc\\Music\\Kerri Chandler\\Album".into(), name: "Album".into(), files, free_slot, queue, speed, hidden: 0 }
    }

    #[test]
    fn search_variations_go_from_the_title_alone_to_simplified_spellings() {
        let v = variations(&want());
        assert_eq!(
            v,
            vec![(1, "Rain".into()), (2, "Rain Original Mix".into()), (3, "Kerri Chandler Rain".into()), (4, "Kerri Chandler Rain Original Mix".into()), (6, "Kerri Chandler".into()), (8, "KCT001".into())],
            "repeats are left out"
        );
        let w = Want { artist: None, title: "Don't Stop (Re-Up)".into(), mix: None, catalogue: None, seconds: None };
        assert_eq!(variations(&w), vec![(1, "Don't Stop Re Up".into()), (9, "dont stop reup".into())]);
        // Several artists, a featured one in the title, accents.
        let w = Want { artist: Some("Beyoncé & Jay-Z".into()), title: "Drunk in Love feat. Someone Else".into(), mix: Some("Remix".into()), catalogue: None, seconds: None };
        let v = variations(&w);
        assert_eq!(v.iter().find(|x| x.0 == 5).map(|x| x.1.as_str()), Some("Beyoncé Drunk in Love"));
        assert_eq!(v.iter().find(|x| x.0 == 6).map(|x| x.1.as_str()), Some("Beyoncé Jay Z"));
        assert_eq!(v.iter().find(|x| x.0 == 10).map(|x| x.1.as_str()), Some("beyonce jayz drunk in love feat someone else"));
        // A long title: its three longest words, in their order.
        let w = Want { artist: Some("Basement Jaxx".into()), title: "The Sound Of The Underground Tonight".into(), mix: None, catalogue: None, seconds: None };
        assert_eq!(variations(&w).iter().find(|x| x.0 == 7).map(|x| x.1.as_str()), Some("Sound Underground Tonight"));
        // One short word finds nothing useful on its own.
        let w = Want { artist: Some("Orbital".into()), title: "Halo".into(), mix: None, catalogue: None, seconds: None };
        assert_eq!(variations(&w).first().map(|x| x.1.as_str()), Some("Halo"));
        let w = Want { artist: Some("Orbital".into()), title: "Lush".into(), mix: Some("3-1".into()), catalogue: None, seconds: None };
        assert!(variations(&w).iter().all(|x| x.1.chars().count() >= 4));
        let w = Want { artist: Some("Underworld".into()), title: "Mmm".into(), mix: None, catalogue: None, seconds: None };
        assert_eq!(variations(&w)[0], (3, "Underworld Mmm".into()), "\"Mmm\" alone is skipped");
        assert_eq!(tokens("Pépé Bradock – Deep Burnt, a 2 B"), ["pepe", "bradock", "deep", "burnt", "2"]);
    }

    #[test]
    fn files_pass_or_fail_each_rule_with_a_reason() {
        let rules = PlaylistRules::default();
        let flac = file("01 Rain (Original Mix)", "flac", 40_000_000, Some(16), Some(44_100), None, Some(432));
        let folders = vec![
            folder("fast", true, 0, 2_000_000, vec![flac.clone()]),
            folder("slow", false, 30, 100_000, vec![flac.clone()]),
            folder("mp3guy", true, 0, 900_000, vec![file("01 Rain (Original Mix)", "mp3", 10_000_000, None, None, Some(320), Some(432))]),
            folder("tiny", true, 0, 900_000, vec![file("01 Rain (Original Mix)", "flac", 3_000_000, Some(16), Some(44_100), None, Some(432))]),
            folder("hires", true, 0, 900_000, vec![file("01 Rain (Original Mix)", "flac", 90_000_000, Some(24), Some(96_000), None, Some(432))]),
            folder("other", true, 0, 900_000, vec![file("03 Rain (Dub Remix)", "flac", 40_000_000, Some(16), Some(44_100), None, Some(400))]),
            folder("wrong", true, 0, 900_000, vec![file("07 Rainbow", "flac", 40_000_000, Some(16), Some(44_100), None, Some(300))]),
        ];
        let c = rank(&folders, &want(), &rules);
        let accepted: Vec<&str> = c.iter().filter(|x| x.rejected.is_none()).map(|x| x.username.as_str()).collect();
        assert_eq!(accepted, ["fast", "hires"], "{c:#?}");
        assert!(sufficient(&c));
        let why = |user: &str| c.iter().find(|x| x.username == user).unwrap().rejected.clone().unwrap();
        assert!(why("slow").contains("queue holds 30"), "{}", why("slow"));
        assert!(why("mp3guy").contains("lossy formats are off"));
        assert!(why("tiny").contains("far smaller"), "{}", why("tiny"));
        assert!(why("other").contains("“remix”"), "{}", why("other"));
        assert!(why("wrong").contains("0% of the title"), "{}", why("wrong"));
        assert!((c[0].overlap - 1.0).abs() < 1e-9);

        // Rules can let lossy in, cap the resolution, and demand a free slot.
        let r = PlaylistRules { lossy: true, max_bit_depth: Some(16), free_slot: true, formats: vec!["flac".into()], ..Default::default() };
        let c = rank(&folders, &want(), &r);
        let accepted: Vec<&str> = c.iter().filter(|x| x.rejected.is_none()).map(|x| x.username.as_str()).collect();
        assert_eq!(accepted, ["fast", "mp3guy"], "lossless before lossy; {c:#?}");
        assert!(c.iter().find(|x| x.username == "hires").unwrap().rejected.as_deref().unwrap().contains("24-bit"));
        assert!(c.iter().find(|x| x.username == "slow").unwrap().rejected.as_deref().unwrap().contains("no free upload slot"));
        let r = PlaylistRules { lossy: true, ..Default::default() };
        let low = vec![folder("low", true, 0, 900_000, vec![file("Rain (Original Mix)", "mp3", 5_000_000, None, None, Some(192), Some(432))])];
        assert!(rank(&low, &want(), &r)[0].rejected.as_deref().unwrap().contains("below the 320 kbps"));
        assert!(!sufficient(&rank(&low, &want(), &r)));
    }

    #[test]
    fn sizes_are_decimal_megabytes_as_everywhere_else() {
        // B18: a 52,000,000-byte file is 52.0 MB in the candidates table, so
        // a 50 MB limit leaves it out, and the reason says 52.0 MB too.
        let rules = PlaylistRules { max_size_mb: Some(50), size_check: false, ..Default::default() };
        let mk = |user: &str, size: i64| folder(user, true, 0, 900_000, vec![file("01 Rain (Original Mix)", "flac", size, Some(16), Some(44_100), None, Some(432))]);
        let c = rank(&[mk("over", 52_000_000), mk("under", 49_900_000)], &want(), &rules);
        let why = c.iter().find(|x| x.username == "over").unwrap().rejected.clone();
        assert_eq!(why.as_deref(), Some("52.0 MB is above the 50 MB limit"));
        assert!(c.iter().find(|x| x.username == "under").unwrap().rejected.is_none());
    }

    #[test]
    fn the_order_follows_format_then_quality_then_the_source() {
        let rules = PlaylistRules::default();
        let w = Want { artist: Some("Artist".into()), title: "Song".into(), mix: None, catalogue: None, seconds: None };
        let mk = |ext: &str, depth: i64, rate: i64| FoundFile { filename: format!("@@x\\Artist\\Song.{ext}"), name: format!("Song.{ext}"), ext: ext.into(), size: 50_000_000, bit_depth: Some(depth), sample_rate: Some(rate), bit_rate: None, length: None, lossless: true };
        let folders = vec![
            FoundFolder { username: "a-wav".into(), directory: "@@x\\Artist".into(), name: "Artist".into(), files: vec![mk("wav", 16, 44_100)], free_slot: true, queue: 0, speed: 5_000_000, hidden: 0 },
            FoundFolder { username: "b-flac-hires-queue".into(), directory: "@@x\\Artist".into(), name: "Artist".into(), files: vec![mk("flac", 24, 96_000)], free_slot: false, queue: 3, speed: 5_000_000, hidden: 0 },
            FoundFolder { username: "c-flac-cd-queue".into(), directory: "@@x\\Artist".into(), name: "Artist".into(), files: vec![mk("flac", 16, 44_100)], free_slot: false, queue: 3, speed: 100_000, hidden: 0 },
            FoundFolder { username: "d-flac-cd-free".into(), directory: "@@x\\Artist".into(), name: "Artist".into(), files: vec![mk("flac", 16, 44_100)], free_slot: true, queue: 0, speed: 100_000, hidden: 0 },
        ];
        let order: Vec<String> = rank(&folders, &w, &rules).iter().map(|c| c.username.clone()).collect();
        assert_eq!(order, ["d-flac-cd-free", "c-flac-cd-queue", "b-flac-hires-queue", "a-wav"]);
        let r = PlaylistRules { quality: "highest".into(), ..Default::default() };
        let order: Vec<String> = rank(&folders, &w, &r).iter().map(|c| c.username.clone()).collect();
        assert_eq!(order[0], "b-flac-hires-queue");
        let c = rank(&folders, &w, &rules);
        let back = Candidate::from_json(&c[0].to_json()).unwrap();
        assert_eq!((back.username.as_str(), back.ext.as_str(), back.rejected), ("d-flac-cd-free", "flac", None));
    }
}
