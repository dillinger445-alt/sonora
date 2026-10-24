//! The track list of a YouTube playlist, read the way a browser reads the
//! playlist page: the `ytInitialData` JSON in its HTML, then the
//! continuation requests the page itself makes until the list is complete.
//! No API key of the person's and no yt-dlp. Only public and unlisted
//! playlists can be read; private lists (Liked videos, Watch Later, a
//! playlist set to private) are told apart and said so.
//!
//! What is sent: the playlist's id, in the page's address and in the
//! continuation requests, with a browser's User-Agent and a cookie that says
//! no consent dialog is wanted. Nothing about the person or their library.
//! Requests go through the system's curl (`net.rs`).
//!
//! Each video's title is then read as artist and title (`parse_title`), the
//! way people write them on YouTube: "Artist - Title (Mix) [Label]",
//! "PREMIERE: …", side markers, hashtags, years and catalogue numbers.

use crate::net::{Http, NetError, Request};
use sonora_json::{obj, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Where playlist pages come from unless `SONORA_YOUTUBE_BASE` says
/// otherwise (a stand-in in tests).
pub const DEFAULT_BASE: &str = "https://www.youtube.com";
const TIMEOUT: Duration = Duration::from_secs(30);
/// Most videos read from one playlist.
pub const MAX_VIDEOS: usize = 5_000;
/// Most continuation requests for one playlist.
const MAX_PAGES: usize = 60;
/// The web client the page would use; replaced by the page's own version.
const CLIENT_VERSION: &str = "2.20261015.01.00";
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15";

pub fn base() -> String {
    std::env::var("SONORA_YOUTUBE_BASE")
        .ok()
        .filter(|b| b.starts_with("http://") || b.starts_with("https://"))
        .map(|b| b.trim_end_matches('/').to_string())
        .unwrap_or_else(|| DEFAULT_BASE.into())
}

#[derive(Clone, Debug, PartialEq)]
pub enum YoutubeError {
    /// Private, or not a playlist that can be read (a Mix, Watch Later).
    NotPublic(String),
    NotFound,
    Unreachable(String),
    /// The page did not hold a playlist SONORA could read.
    Unreadable(String),
    Cancelled,
}

impl std::fmt::Display for YoutubeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            YoutubeError::NotPublic(m) => f.write_str(m),
            YoutubeError::NotFound => f.write_str("YouTube has no playlist with that id. Check the link: it needs the list= part, as in youtube.com/playlist?list=…"),
            YoutubeError::Unreachable(m) => write!(f, "YouTube could not be reached: {m}"),
            YoutubeError::Unreadable(m) => write!(f, "The playlist page could not be read: {m}"),
            YoutubeError::Cancelled => f.write_str("Stopped."),
        }
    }
}

impl YoutubeError {
    pub fn code(&self) -> &'static str {
        match self {
            YoutubeError::NotPublic(_) => "playlist_private",
            YoutubeError::NotFound => "playlist_not_found",
            YoutubeError::Unreachable(_) => "youtube_unreachable",
            YoutubeError::Unreadable(_) => "playlist_unreadable",
            YoutubeError::Cancelled => "cancelled",
        }
    }
}

/// The playlist id in a link (youtube.com/playlist?list=…, a watch link
/// with list=, music.youtube.com) or typed on its own. Lists private to an
/// account (Watch Later, Liked videos) and Mixes are refused with a reason.
pub fn playlist_id(input: &str) -> Result<String, String> {
    let s = input.trim();
    if s.is_empty() {
        return Err("Paste the link of a YouTube playlist.".into());
    }
    let id = match s.find("list=") {
        Some(i) => s[i + 5..].split(['&', '#', '?', ' ', '\n']).next().unwrap_or("").to_string(),
        None if s.contains("://") || s.contains("youtube.") || s.contains("youtu.be") || s.contains('/') => {
            return Err("That link has no playlist in it. A playlist link has a list= part, as in youtube.com/playlist?list=…".into());
        }
        None => s.to_string(),
    };
    match id.as_str() {
        "WL" => return Err("Watch Later is private to your account and cannot be fetched. Put the videos in a playlist that is public or unlisted.".into()),
        "LL" | "LM" => return Err("Liked videos are private to your account and cannot be fetched. Put the videos in a playlist that is public or unlisted.".into()),
        _ if id.starts_with("RD") => return Err("That is a Mix YouTube makes for you, not a playlist, so it cannot be fetched. Put the videos in a playlist that is public or unlisted.".into()),
        _ => {}
    }
    if id.len() < 10 || id.len() > 64 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return Err("That does not look like a YouTube playlist id.".into());
    }
    Ok(id)
}

// --------------------------------------------------------------- the page

/// One video as the playlist lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct Video {
    pub id: String,
    pub title: String,
    pub channel: String,
    pub seconds: Option<i64>,
    /// False for deleted and private videos.
    pub playable: bool,
}

/// What one page (or one continuation) of the playlist holds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page {
    pub title: Option<String>,
    pub videos: Vec<Video>,
    pub continuation: Option<String>,
    /// What YouTube says instead of a list ("This playlist is private.").
    pub alert: Option<String>,
}

/// The JSON object assigned to `ytInitialData` in the page.
pub fn initial_data(html: &str) -> Option<Value> {
    let at = html.find("ytInitialData")?;
    let rest = &html[at..];
    let eq = rest.find('=')?;
    let after = &rest[eq + 1..];
    let start = after.find('{')?;
    let json = &after[start..];
    let end = object_end(json)?;
    sonora_json::parse(&json[..end]).ok()
}

/// Where the JSON object starting at `s[0]` ends (strings and escapes respected).
fn object_end(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let (mut depth, mut in_str, mut esc) = (0usize, false, false);
    for (i, &c) in b.iter().enumerate() {
        if in_str {
            if esc {
                esc = false;
            } else if c == b'\\' {
                esc = true;
            } else if c == b'"' {
                in_str = false;
            }
            continue;
        }
        match c {
            b'"' => in_str = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// A quoted value in the page's `ytcfg` ("INNERTUBE_API_KEY", say).
fn cfg_value(html: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\":\"");
    let at = html.find(&needle)? + needle.len();
    let v = html[at..].split('"').next()?;
    (!v.is_empty() && v.len() < 200 && v.chars().all(|c| c.is_ascii_graphic())).then(|| v.to_string())
}

/// The text of a YouTube text node: `simpleText`, `runs[].text`, or a
/// view model's `content`.
fn text(v: Option<&Value>) -> String {
    let Some(v) = v else { return String::new() };
    if let Some(s) = v.as_str() {
        return s.to_string();
    }
    if let Some(s) = v.get("simpleText").and_then(Value::as_str) {
        return s.to_string();
    }
    if let Some(s) = v.get("content").and_then(Value::as_str) {
        return s.to_string();
    }
    if let Some(runs) = v.get("runs").and_then(Value::as_array) {
        return runs.iter().filter_map(|r| r.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("");
    }
    if let Some(t) = v.get("text") {
        return text(Some(t));
    }
    String::new()
}

enum Item<'a> {
    Video(&'a Value),
    Continuation(String),
}

/// Every video renderer and continuation token, in the order the page has
/// them, wherever YouTube's layout puts them.
fn collect<'a>(v: &'a Value, out: &mut Vec<Item<'a>>) {
    match v {
        Value::Object(map) => {
            if let Some(r) = map.get("playlistVideoRenderer") {
                out.push(Item::Video(r));
                return;
            }
            if let Some(r) = map.get("continuationItemRenderer") {
                let token = r.get("continuationEndpoint").and_then(|e| e.get("continuationCommand")).and_then(|c| c.get("token")).and_then(Value::as_str);
                if let Some(t) = token.filter(|t| !t.is_empty()) {
                    out.push(Item::Continuation(t.to_string()));
                }
                return;
            }
            for (_, child) in map.iter() {
                collect(child, out);
            }
        }
        Value::Array(a) => {
            for child in a {
                collect(child, out);
            }
        }
        _ => {}
    }
}

fn video(r: &Value) -> Option<Video> {
    let id = r.get("videoId").and_then(Value::as_str)?.to_string();
    if id.is_empty() {
        return None;
    }
    let seconds = r
        .get("lengthSeconds")
        .and_then(|s| s.as_str().and_then(|t| t.parse::<i64>().ok()).or_else(|| s.as_i64()))
        .filter(|s| *s > 0)
        .or_else(|| parse_clock(&text(r.get("lengthText"))));
    let channel = text(r.get("shortBylineText"));
    let playable = r.get("isPlayable").and_then(Value::as_bool).unwrap_or(true);
    Some(Video { id, title: text(r.get("title")), channel, seconds, playable })
}

/// "7:12" or "1:03:05" in seconds.
fn parse_clock(s: &str) -> Option<i64> {
    let parts: Vec<i64> = s.trim().split(':').map(|p| p.trim().parse::<i64>().ok()).collect::<Option<Vec<_>>>()?;
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }
    Some(parts.iter().fold(0, |acc, p| acc * 60 + p)).filter(|s| *s > 0)
}

/// The videos, the next continuation token and the alerts in a page's data.
pub fn parse_page(data: &Value) -> Page {
    let mut items = Vec::new();
    collect(data, &mut items);
    let mut page = Page::default();
    for item in items {
        match item {
            Item::Video(r) => {
                if let Some(v) = video(r) {
                    page.videos.push(v);
                }
            }
            Item::Continuation(t) => {
                if page.continuation.is_none() {
                    page.continuation = Some(t);
                }
            }
        }
    }
    let title = [
        data.get("metadata").and_then(|m| m.get("playlistMetadataRenderer")).and_then(|m| m.get("title")),
        data.get("microformat").and_then(|m| m.get("microformatDataRenderer")).and_then(|m| m.get("title")),
        data.get("header").and_then(|h| h.get("playlistHeaderRenderer")).and_then(|h| h.get("title")),
        data.get("header").and_then(|h| h.get("pageHeaderRenderer")).and_then(|h| h.get("pageTitle")),
    ]
    .into_iter()
    .map(text)
    .find(|t| !t.trim().is_empty());
    page.title = title.map(|t| t.trim().to_string());
    if let Some(alerts) = data.get("alerts").and_then(Value::as_array) {
        for a in alerts {
            let r = a.get("alertRenderer").or_else(|| a.get("alertWithButtonRenderer"));
            let t = text(r.and_then(|r| r.get("text")));
            if !t.trim().is_empty() {
                page.alert = Some(t.trim().to_string());
                break;
            }
        }
    }
    page
}

pub fn parse_html(html: &str) -> Result<Page, YoutubeError> {
    let data = initial_data(html).ok_or_else(|| YoutubeError::Unreadable("the page holds no playlist data (YouTube may have changed its page, or answered with something else)".into()))?;
    Ok(parse_page(&data))
}

pub fn parse_continuation(json: &str) -> Result<Page, YoutubeError> {
    let data = sonora_json::parse(json).map_err(|_| YoutubeError::Unreadable("YouTube's answer to a continuation request was not JSON".into()))?;
    Ok(parse_page(&data))
}

/// A whole playlist.
#[derive(Clone, Debug, PartialEq)]
pub struct Playlist {
    pub id: String,
    pub title: String,
    pub videos: Vec<Video>,
    /// The list was longer than `MAX_VIDEOS`.
    pub truncated: bool,
}

fn net_error(e: NetError) -> YoutubeError {
    match e {
        NetError::NoCurl(m) | NetError::Failed(m) => YoutubeError::Unreachable(m),
        NetError::Unreachable(m) => YoutubeError::Unreachable(format!("{m}. Is the computer connected to the internet?")),
        NetError::Certificate(m) => YoutubeError::Unreachable(format!("its certificate could not be checked ({m})")),
        NetError::TimedOut => YoutubeError::Unreachable("it did not answer in time".into()),
    }
}

fn common_headers() -> Vec<(String, String)> {
    vec![
        ("User-Agent".into(), USER_AGENT.into()),
        ("Accept-Language".into(), "en-US,en;q=0.9".into()),
        // No consent dialog in place of the page (the EU's), and no identity.
        ("Cookie".into(), "SOCS=CAI".into()),
    ]
}

/// Reads the whole playlist: the page, then its continuations. `progress`
/// gets the number of videos read so far.
pub fn fetch(http: &dyn Http, base: &str, id: &str, cancel: &AtomicBool, progress: &mut dyn FnMut(usize)) -> Result<Playlist, YoutubeError> {
    let mut headers = common_headers();
    headers.push(("Accept".into(), "text/html,application/xhtml+xml".into()));
    let req = Request { method: "GET", url: format!("{base}/playlist?list={id}&hl=en"), headers, body: None, timeout: TIMEOUT };
    let r = http.send(&req).map_err(net_error)?;
    match r.status {
        200..=299 => {}
        404 => return Err(YoutubeError::NotFound),
        429 => return Err(YoutubeError::Unreachable("YouTube asked for a pause (too many requests). Try again in a few minutes.".into())),
        s => return Err(YoutubeError::Unreachable(format!("YouTube answered with status {s}"))),
    }
    let page = parse_html(&r.body)?;
    if page.videos.is_empty() && page.continuation.is_none() {
        if let Some(alert) = page.alert {
            let lower = alert.to_lowercase();
            if lower.contains("does not exist") || lower.contains("not exist") {
                return Err(YoutubeError::NotFound);
            }
            if lower.contains("private") {
                return Err(YoutubeError::NotPublic("This playlist is private. On YouTube, set it to public or unlisted (Edit → Visibility), then try again.".into()));
            }
            return Err(YoutubeError::NotPublic(format!("YouTube says: {alert}")));
        }
    }
    let title = page.title.clone().unwrap_or_else(|| "YouTube playlist".into());
    let api_key = cfg_value(&r.body, "INNERTUBE_API_KEY");
    let version = cfg_value(&r.body, "INNERTUBE_CLIENT_VERSION").unwrap_or_else(|| CLIENT_VERSION.into());
    let mut videos = page.videos;
    let mut token = page.continuation;
    let mut truncated = false;
    let mut pages = 0;
    progress(videos.len());
    while let Some(t) = token.take() {
        if cancel.load(Ordering::Relaxed) {
            return Err(YoutubeError::Cancelled);
        }
        pages += 1;
        if pages > MAX_PAGES || videos.len() >= MAX_VIDEOS {
            truncated = true;
            break;
        }
        let mut headers = common_headers();
        headers.push(("X-YouTube-Client-Name".into(), "1".into()));
        headers.push(("X-YouTube-Client-Version".into(), version.clone()));
        headers.push(("Origin".into(), base.to_string()));
        let key = api_key.as_ref().map(|k| format!("&key={k}")).unwrap_or_default();
        let body = obj! {
            "context" => obj! { "client" => obj! { "clientName" => "WEB", "clientVersion" => version.clone(), "hl" => "en", "gl" => "US" } },
            "continuation" => t,
        };
        let req = Request { method: "POST", url: format!("{base}/youtubei/v1/browse?prettyPrint=false{key}"), headers, body: Some(body.to_string_compact()), timeout: TIMEOUT };
        let r = http.send(&req).map_err(net_error)?;
        if !(200..300).contains(&r.status) {
            return Err(YoutubeError::Unreachable(format!("YouTube answered a continuation request with status {}", r.status)));
        }
        let more = parse_continuation(&r.body)?;
        if more.videos.is_empty() && more.continuation.is_none() {
            break;
        }
        videos.extend(more.videos);
        token = more.continuation;
        progress(videos.len());
    }
    if videos.len() > MAX_VIDEOS {
        videos.truncate(MAX_VIDEOS);
        truncated = true;
    }
    Ok(Playlist { id: id.to_string(), title, videos, truncated })
}

// -------------------------------------------------------------- titles

/// A video's title read as a track.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Parsed {
    pub artist: Option<String>,
    pub title: String,
    /// The mix name from the title: "Original Mix", "Carl Craig Remix".
    pub mix: Option<String>,
    pub featuring: Option<String>,
    /// A catalogue number from the title: "HG007".
    pub catalogue: Option<String>,
    pub year: Option<i64>,
    /// A label name in square brackets, left out of the title.
    pub label: Option<String>,
    /// The title carried #shorts.
    pub shorts: bool,
}

const MIX_WORDS: &[&str] = &[
    "mix", "remix", "rmx", "edit", "dub", "version", "rework", "remaster", "remastered", "bootleg", "vip", "instrumental", "extended", "original", "radio", "club", "live", "acoustic", "flip", "refix",
    "rerub", "re-edit", "reedit", "cut", "take", "demo", "mixx", "vocal", "beats", "acapella", "mashup",
];
const NOISE_WORDS: &[&str] = &[
    "official", "video", "audio", "hq", "hd", "4k", "lyric", "lyrics", "visualizer", "visualiser", "full", "track", "snippet", "preview", "clip", "free", "download", "out", "now", "premiere", "stream", "teaser",
    "promo", "music", "mv", "new", "exclusive", "upload", "release", "available", "on", "vinyl", "digital", "only", "hi-res", "stereo", "mono", "720p", "1080p", "high", "quality", "sound", "remastered",
    "with", "excerpt", "edit", "taster", "ad", "advert", "ep", "lp", "album", "single",
];

fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric() && c != '-' && c != '\'').filter(|w| !w.is_empty()).map(|w| w.to_lowercase()).collect()
}

/// Every word is one a video title carries about the video, not the music.
fn is_noise(s: &str) -> bool {
    let w = words(s);
    !w.is_empty() && w.iter().all(|x| NOISE_WORDS.contains(&x.as_str()) || x.chars().all(|c| c.is_ascii_digit()) && x.len() <= 2)
}

fn is_mix(s: &str) -> bool {
    words(s).iter().any(|x| MIX_WORDS.contains(&x.as_str()))
}

fn is_year(s: &str) -> Option<i64> {
    let t = s.trim();
    (t.len() == 4 && (t.starts_with("19") || t.starts_with("20")) && t.chars().all(|c| c.is_ascii_digit())).then(|| t.parse().ok()).flatten()
}

/// "HG007", "KS 030", "FFRR-123", "430WEST001".
fn is_catalogue(s: &str) -> bool {
    let t: String = s.trim().chars().filter(|c| !c.is_whitespace() && *c != '-' && *c != '_' && *c != '.').collect();
    if t.len() < 4 || t.len() > 14 || !t.chars().all(|c| c.is_ascii_alphanumeric()) {
        return false;
    }
    let letters = t.chars().take_while(|c| c.is_ascii_alphabetic()).count();
    let rest = &t[letters..];
    let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
    let tail = &rest[digits..];
    (2..=6).contains(&letters) && (2..=5).contains(&digits) && tail.len() <= 2 && tail.chars().all(|c| c.is_ascii_alphanumeric())
}

fn featuring_split(s: &str) -> (String, Option<String>) {
    let lower = s.to_lowercase();
    for marker in [" feat. ", " feat ", " ft. ", " ft ", " featuring ", " w/ "] {
        if let Some(i) = lower.find(marker) {
            let main = s[..i].trim().to_string();
            let who = s[i + marker.len()..].trim().trim_matches(|c: char| c == '-' || c == '–' || c == '—' || c == ' ').to_string();
            return (main, (!who.is_empty()).then_some(who));
        }
    }
    (s.trim().to_string(), None)
}

/// A group in brackets or parentheses, classified.
enum Group {
    Catalogue(String),
    Year(i64),
    Featuring(String),
    Mix(String),
    Noise,
    Label(String),
    Keep,
}

fn classify(inner: &str, square: bool) -> Group {
    let t = inner.trim().trim_matches(|c: char| c == '"' || c == '“' || c == '”');
    if t.is_empty() {
        return Group::Noise;
    }
    let lower = t.to_lowercase();
    if lower.starts_with("feat") || lower.starts_with("ft.") || lower.starts_with("ft ") || lower.starts_with("featuring") || lower.starts_with("w/") {
        let (_, who) = featuring_split(&format!(" {t}"));
        return who.map(Group::Featuring).unwrap_or(Group::Noise);
    }
    if let Some(y) = is_year(t) {
        return Group::Year(y);
    }
    if is_catalogue(t) {
        return Group::Catalogue(t.replace(' ', "").to_uppercase());
    }
    if is_noise(t) {
        return Group::Noise;
    }
    if is_mix(t) {
        return Group::Mix(t.to_string());
    }
    if square {
        return Group::Label(t.to_string());
    }
    Group::Keep
}

/// Splits on " - ", " – ", " — " and on a dash without spaces.
fn segments(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let spaced_hyphen = c == '-' && i > 0 && chars[i - 1] == ' ' && i + 1 < chars.len() && chars[i + 1] == ' ';
        if c == '–' || c == '—' || spaced_hyphen {
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
        i += 1;
    }
    out.push(cur);
    out.into_iter().map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect()
}

/// "A1.", "B2 -", "01." at the start of a title or a segment: a side of a
/// record (one letter, one digit) or a track number (one or two digits with
/// a dot or a bracket). "2 Become 1" and the band "B12" are left alone.
fn strip_marker(s: &str) -> String {
    let t = s.trim_start();
    let b: Vec<char> = t.chars().collect();
    let side = b.len() > 2 && b[0].is_ascii_alphabetic() && b[0].to_ascii_uppercase() <= 'H' && b[1].is_ascii_digit() && b[1] != '0';
    let (after, needs_sep) = if side {
        (2, false)
    } else {
        let digits = b.iter().take_while(|c| c.is_ascii_digit()).count();
        if digits == 0 || digits > 2 {
            return t.to_string();
        }
        (digits, true)
    };
    let seps = b[after..].iter().take_while(|c| matches!(c, '.' | ')' | ':' | ' ' | '-' | '–' | '—' | '_')).count();
    let punctuated = b[after..after + seps].iter().any(|c| *c != ' ');
    if after + seps < b.len() && seps > 0 && (punctuated || !needs_sep) {
        return b[after + seps..].iter().collect::<String>().trim().to_string();
    }
    t.to_string()
}

/// A segment that is only a side marker ("B2") or a track number ("01").
fn is_marker_only(s: &str) -> bool {
    let b: Vec<char> = s.trim().chars().collect();
    match b.as_slice() {
        [a, d] => a.is_ascii_alphabetic() && a.to_ascii_uppercase() <= 'H' && d.is_ascii_digit(),
        all => !all.is_empty() && all.len() <= 2 && all.iter().all(|c| c.is_ascii_digit()),
    }
}

fn tidy(s: &str) -> String {
    let t = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let t = t.trim_matches(|c: char| matches!(c, '-' | '–' | '—' | ':' | '|' | ',' | ' ' | '"' | '“' | '”' | '\'' | '‘' | '’' | '.' | '~' | '*'));
    // Quotes around the whole thing only; an apostrophe inside stays.
    t.to_string()
}

/// Reads a video title as artist and title. `channel` fills in the artist
/// for "Artist - Topic" channels when the title has none.
pub fn parse_title(raw: &str, channel: &str) -> Parsed {
    let mut p = Parsed::default();
    let mut t = raw.trim().to_string();
    let lower = t.to_lowercase();
    p.shorts = lower.contains("#shorts") || lower.contains("#short ");
    // "PREMIERE: …", "Premiere | …"
    let lower = t.to_lowercase();
    if lower.starts_with("premiere") {
        let rest = t[8..].trim_start_matches([':', '|', '-', '–', '—', ' ', '!']);
        t = rest.to_string();
    }
    // Hashtags (#deephouse), but not "#1".
    t = t
        .split(' ')
        .filter(|w| !(w.starts_with('#') && w.chars().nth(1).is_some_and(|c| c.is_alphabetic())))
        .collect::<Vec<_>>()
        .join(" ");
    // Groups in parentheses and brackets.
    let mut out = String::new();
    let chars: Vec<char> = t.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let close = match c {
            '(' => Some(')'),
            '[' => Some(']'),
            '{' => Some('}'),
            _ => None,
        };
        if let Some(close) = close {
            if let Some(len) = chars[i + 1..].iter().position(|&x| x == close) {
                let inner: String = chars[i + 1..i + 1 + len].iter().collect();
                match classify(&inner, c != '(') {
                    Group::Catalogue(n) => p.catalogue = p.catalogue.or(Some(n)),
                    Group::Year(y) => p.year = p.year.or(Some(y)),
                    Group::Featuring(w) => p.featuring = p.featuring.or(Some(w)),
                    Group::Mix(m) => p.mix = Some(match p.mix.take() {
                        Some(old) => format!("{old} {m}"),
                        None => m,
                    }),
                    Group::Noise => {}
                    Group::Label(l) => p.label = p.label.or(Some(l)),
                    Group::Keep => {
                        out.push(c);
                        out.push_str(&inner);
                        out.push(close);
                    }
                }
                out.push(' ');
                i += len + 2;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    // "Artist - Title | Label": what follows a bar is about the video.
    let t = out.split('|').next().unwrap_or("").to_string();
    let t = strip_marker(&t);
    let mut segs: Vec<String> = segments(&t).into_iter().map(|s| strip_marker(&s)).filter(|s| !s.is_empty()).collect();
    segs.retain(|s| {
        if is_marker_only(s) || is_noise(s) {
            return false;
        }
        if let Some(y) = is_year(s) {
            if p.year.is_none() {
                p.year = Some(y);
            }
            return false;
        }
        if is_catalogue(s) && s.chars().any(|c| c.is_ascii_digit()) && s.chars().filter(|c| c.is_alphabetic()).all(|c| c.is_uppercase()) {
            if p.catalogue.is_none() {
                p.catalogue = Some(s.replace(' ', "").to_uppercase());
            }
            return false;
        }
        true
    });
    let topic = channel.trim().strip_suffix(" - Topic").map(str::trim).filter(|a| !a.is_empty());
    match segs.len() {
        0 => {
            p.title = tidy(raw);
            p.artist = topic.map(str::to_string);
        }
        1 => {
            let (title, feat) = featuring_split(&segs[0]);
            p.featuring = p.featuring.or(feat);
            p.title = tidy(&title);
            p.artist = topic.map(str::to_string);
        }
        _ => {
            let (artist, feat) = featuring_split(&segs[0]);
            p.featuring = p.featuring.or(feat);
            p.artist = Some(tidy(&artist)).filter(|a| !a.is_empty());
            let (title, feat) = featuring_split(&segs[1]);
            p.featuring = p.featuring.or(feat);
            p.title = tidy(&title);
            // "Artist - Title - Carl Craig Remix": a third part that names a mix.
            for extra in &segs[2..] {
                if is_mix(extra) && p.mix.is_none() {
                    p.mix = Some(tidy(extra));
                }
            }
        }
    }
    if p.title.is_empty() {
        p.title = tidy(raw);
    }
    p
}

/// Letters, digits and single spaces, lower case, common accents folded:
/// the form two titles are compared in.
pub fn norm(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = true;
    for c in s.chars() {
        let folded: &str = match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' => "a",
            'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' => "a",
            'ç' | 'Ç' | 'č' | 'ć' => "c",
            'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ė' | 'È' | 'É' | 'Ê' | 'Ë' => "e",
            'ì' | 'í' | 'î' | 'ï' | 'Ì' | 'Í' | 'Î' | 'Ï' => "i",
            'ñ' | 'Ñ' | 'ń' => "n",
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' | 'Ø' => "o",
            'ù' | 'ú' | 'û' | 'ü' | 'Ù' | 'Ú' | 'Û' | 'Ü' => "u",
            'ý' | 'ÿ' | 'Ý' => "y",
            'ß' => "ss",
            'æ' | 'Æ' => "ae",
            'œ' | 'Œ' => "oe",
            'š' | 'Š' => "s",
            'ž' | 'Ž' => "z",
            '&' => " and ",
            _ => "",
        };
        if !folded.is_empty() {
            for fc in folded.chars() {
                if fc == ' ' {
                    if !space {
                        out.push(' ');
                        space = true;
                    }
                } else {
                    out.push(fc);
                    space = false;
                }
            }
            continue;
        }
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
            space = false;
        } else if !space {
            out.push(' ');
            space = true;
        }
    }
    out.trim().to_string()
}

/// Why a video is left out of the tracks to look for, or `None` to keep it.
/// The person can put any of them back.
pub fn leave_out(v: &Video, p: &Parsed) -> Option<String> {
    let lower = v.title.trim().to_lowercase();
    if !v.playable || lower == "[deleted video]" || lower == "[private video]" {
        return Some("This video is no longer available.".into());
    }
    if p.shorts {
        return Some("A Short, not a track.".into());
    }
    if let Some(s) = v.seconds {
        if s > 30 * 60 {
            return Some("Over 30 minutes: a mix or a set rather than one track.".into());
        }
        if s < 120 && p.artist.is_none() {
            return Some("Under 2 minutes, and the title does not name an artist.".into());
        }
    }
    None
}

/// The key two videos of the same track share: artist, title and mix name, normalised.
pub fn track_key(p: &Parsed) -> String {
    format!("{}\n{}\n{}", norm(p.artist.as_deref().unwrap_or("")), norm(&p.title), norm(p.mix.as_deref().unwrap_or("")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::Response;
    use std::sync::Mutex;

    const PLAYLIST: &str = include_str!("../tests/fixtures/youtube/playlist.html");
    const CONTINUATION: &str = include_str!("../tests/fixtures/youtube/continuation.json");
    const PRIVATE: &str = include_str!("../tests/fixtures/youtube/private.html");

    #[test]
    fn playlist_ids_come_from_links_and_private_lists_are_refused() {
        assert_eq!(playlist_id("https://www.youtube.com/playlist?list=PLfixtureLateNight").unwrap(), "PLfixtureLateNight");
        assert_eq!(playlist_id("https://youtube.com/watch?v=a1b2c3d4e5F&list=PLfixtureLateNight&index=2").unwrap(), "PLfixtureLateNight");
        assert_eq!(playlist_id(" https://music.youtube.com/playlist?list=OLAK5uy_kfixture123456789&si=abc ").unwrap(), "OLAK5uy_kfixture123456789");
        assert_eq!(playlist_id("PLfixtureLateNight").unwrap(), "PLfixtureLateNight");
        assert!(playlist_id("https://www.youtube.com/watch?v=a1b2c3d4e5F").unwrap_err().contains("list="));
        assert!(playlist_id("not a list").is_err());
        assert!(playlist_id("").is_err());
        assert!(playlist_id("https://www.youtube.com/playlist?list=WL").unwrap_err().contains("Watch Later"));
        assert!(playlist_id("LL").unwrap_err().contains("Liked videos"));
        assert!(playlist_id("https://www.youtube.com/watch?v=x&list=RDa1b2c3d4e5F").unwrap_err().contains("Mix"));
    }

    #[test]
    fn the_page_gives_videos_title_and_continuation() {
        let page = parse_html(PLAYLIST).unwrap();
        assert_eq!(page.title.as_deref(), Some("Late Night Selections"));
        assert_eq!(page.videos.len(), 8);
        let first = &page.videos[0];
        assert_eq!((first.id.as_str(), first.title.as_str(), first.channel.as_str(), first.seconds, first.playable), ("a1b2c3d4e5F", "Kerri Chandler - Rain (Original Mix)", "Kerri Chandler - Topic", Some(432), true));
        assert_eq!(page.continuation.as_deref(), Some("4qmFsgKBARIS...fixture-continuation-token-1"));
        assert_eq!(page.alert, None);
        assert_eq!(cfg_value(PLAYLIST, "INNERTUBE_API_KEY").as_deref(), Some("AIzaSyFixtureKey0000000000000000000000"));
        assert_eq!(cfg_value(PLAYLIST, "INNERTUBE_CLIENT_VERSION").as_deref(), Some("2.20261015.01.00"));

        let more = parse_continuation(CONTINUATION).unwrap();
        assert_eq!(more.videos.len(), 5);
        assert_eq!(more.continuation, None);
        assert!(!more.videos[1].playable, "a deleted video");
        assert_eq!(more.videos[3].seconds, Some(10_801));

        let private = parse_html(PRIVATE).unwrap();
        assert!(private.videos.is_empty());
        assert_eq!(private.alert.as_deref(), Some("This playlist is private."));
        assert!(parse_html("<html><body>nothing here</body></html>").is_err());
        assert_eq!(object_end(r#"{"a":"}{","b":{"c":[1,2]}} tail"#), Some(26));
        assert_eq!(parse_clock("1:03:05"), Some(3785));
        assert_eq!(parse_clock("7:12"), Some(432));
    }

    /// A stand-in YouTube that serves the fixtures and records what was asked.
    struct FakeTube {
        requests: Mutex<Vec<Request>>,
        /// The status the playlist page answers with.
        status: u16,
        html: &'static str,
    }

    impl Http for FakeTube {
        fn send(&self, req: &Request) -> Result<Response, NetError> {
            self.requests.lock().unwrap().push(req.clone());
            if req.url.contains("/playlist?list=") {
                return Ok(Response { status: self.status, body: self.html.to_string() });
            }
            if req.url.contains("/youtubei/v1/browse") {
                let body = req.body.as_deref().unwrap_or("");
                assert!(body.contains("fixture-continuation-token-1"), "{body}");
                return Ok(Response { status: 200, body: CONTINUATION.to_string() });
            }
            Ok(Response { status: 404, body: String::new() })
        }
    }

    #[test]
    fn the_whole_list_is_fetched_through_continuations() {
        let tube = FakeTube { requests: Mutex::new(Vec::new()), status: 200, html: PLAYLIST };
        let cancel = AtomicBool::new(false);
        let mut seen = Vec::new();
        let list = fetch(&tube, "https://tube.test", "PLfixtureLateNight", &cancel, &mut |n| seen.push(n)).unwrap();
        assert_eq!(list.title, "Late Night Selections");
        assert_eq!(list.videos.len(), 13);
        assert_eq!(seen, vec![8, 13]);
        assert!(!list.truncated);
        let reqs = tube.requests.lock().unwrap();
        assert_eq!(reqs.len(), 2);
        assert_eq!(reqs[0].method, "GET");
        assert_eq!(reqs[0].url, "https://tube.test/playlist?list=PLfixtureLateNight&hl=en");
        assert!(reqs[0].headers.iter().any(|(k, v)| k == "User-Agent" && v.starts_with("Mozilla")));
        assert_eq!(reqs[1].method, "POST");
        assert!(reqs[1].url.starts_with("https://tube.test/youtubei/v1/browse?prettyPrint=false&key=AIzaSyFixtureKey"), "{}", reqs[1].url);
        assert!(reqs[1].headers.iter().any(|(k, v)| k == "X-YouTube-Client-Version" && v == "2.20261015.01.00"));
        let body = sonora_json::parse(reqs[1].body.as_deref().unwrap()).unwrap();
        assert_eq!(body.get("context").and_then(|c| c.get("client")).and_then(|c| c.get("clientName")).and_then(Value::as_str), Some("WEB"));
        // Nothing about the person goes along: no cookies but the consent one, no account.
        for r in reqs.iter() {
            let cookie = r.headers.iter().find(|(k, _)| k == "Cookie").map(|(_, v)| v.as_str());
            assert_eq!(cookie, Some("SOCS=CAI"));
            assert!(!r.headers.iter().any(|(k, _)| k == "Authorization"));
        }
    }

    #[test]
    fn private_and_missing_playlists_are_told_apart() {
        let tube = FakeTube { requests: Mutex::new(Vec::new()), status: 200, html: PRIVATE };
        let cancel = AtomicBool::new(false);
        let e = fetch(&tube, "https://tube.test", "PLprivate0000", &cancel, &mut |_| {}).unwrap_err();
        assert!(matches!(&e, YoutubeError::NotPublic(m) if m.contains("private") && m.contains("unlisted")), "{e:?}");
        assert_eq!(e.code(), "playlist_private");
        let tube = FakeTube { requests: Mutex::new(Vec::new()), status: 404, html: "" };
        assert_eq!(fetch(&tube, "https://tube.test", "PLgone00000000", &cancel, &mut |_| {}).unwrap_err(), YoutubeError::NotFound);
        let stopped = AtomicBool::new(true);
        let tube = FakeTube { requests: Mutex::new(Vec::new()), status: 200, html: PLAYLIST };
        assert_eq!(fetch(&tube, "https://tube.test", "PLfixtureLateNight", &stopped, &mut |_| {}).unwrap_err(), YoutubeError::Cancelled);
    }

    fn parsed(title: &str, channel: &str) -> (Option<String>, String, Option<String>) {
        let p = parse_title(title, channel);
        (p.artist, p.title, p.mix)
    }

    #[test]
    fn titles_are_read_as_artist_and_title() {
        let s = |x: &str| Some(x.to_string());
        assert_eq!(parsed("Kerri Chandler - Rain (Original Mix)", "Kerri Chandler - Topic"), (s("Kerri Chandler"), "Rain".into(), s("Original Mix")));
        assert_eq!(parsed("PREMIERE: Octave One – Blackwater [430 West] (Official Video)", "Some Channel"), (s("Octave One"), "Blackwater".into(), None));
        let p = parse_title("Moodymann — Shades Of Jae (1998)", "deephousevault");
        assert_eq!((p.artist.as_deref(), p.title.as_str(), p.year), (Some("Moodymann"), "Shades Of Jae", Some(1998)));
        let p = parse_title("A1. Theo Parrish - Falling Up (Carl Craig Remix) [SOUND SIGNATURE] #deephouse #detroit", "RecordDigger");
        assert_eq!((p.artist.as_deref(), p.title.as_str(), p.mix.as_deref(), p.label.as_deref()), (Some("Theo Parrish"), "Falling Up", Some("Carl Craig Remix"), Some("SOUND SIGNATURE")));
        assert!(!p.shorts);
        assert_eq!(parsed("Don't Go (Extended Mix)", "Floorplan - Topic"), (s("Floorplan"), "Don't Go".into(), s("Extended Mix")));
        let p = parse_title("Pépé Bradock feat. Shedding Skin - Deep Burnt - Preview", "Kif Recordings");
        assert_eq!((p.artist.as_deref(), p.title.as_str(), p.featuring.as_deref()), (Some("Pépé Bradock"), "Deep Burnt", Some("Shedding Skin")));
        assert_eq!(parsed("Intro", "RecordDigger"), (None, "Intro".into(), None));
        let p = parse_title("Ron Trent ft. Lady Alma - Morning Factory (Full Track)", "Prescription");
        assert_eq!((p.artist.as_deref(), p.title.as_str(), p.featuring.as_deref()), (Some("Ron Trent"), "Morning Factory", Some("Lady Alma")));
        let p = parse_title("Larry Heard - The Sun Can't Compare (Long Version) #shorts", "clips");
        assert_eq!((p.artist.as_deref(), p.title.as_str(), p.mix.as_deref(), p.shorts), (Some("Larry Heard"), "The Sun Can't Compare", Some("Long Version"), true));
        let p = parse_title("B2 - Rick Wade - Night Driving (Radio Edit) [HG007] - 2004", "Harmonie Park");
        assert_eq!((p.artist.as_deref(), p.title.as_str(), p.mix.as_deref(), p.catalogue.as_deref(), p.year), (Some("Rick Wade"), "Night Driving", Some("Radio Edit"), Some("HG007"), Some(2004)));
        assert_eq!(parsed("Deep House Mix 2026 | 3 Hours of Late Night Grooves", "Mix Channel"), (None, "Deep House Mix 2026".into(), None));
        assert_eq!(parsed("Artist - Title - Carl Craig Remix", "x"), (s("Artist"), "Title".into(), s("Carl Craig Remix")));
        assert_eq!(parsed("01. Some Artist - Some Title", "x"), (s("Some Artist"), "Some Title".into(), None));
        assert_eq!(parsed("Artist - \"Quoted Title\" (feat. Someone) [CAT 012]", "x"), (s("Artist"), "Quoted Title".into(), None));
        let p = parse_title("Artist - Title (feat. Someone) [CAT 012]", "x");
        assert_eq!((p.featuring.as_deref(), p.catalogue.as_deref()), (Some("Someone"), Some("CAT012")));
        // Words that happen to start like a marker are kept.
        assert_eq!(parsed("2 Become 1", "Spice Girls - Topic"), (s("Spice Girls"), "2 Become 1".into(), None));
        assert_eq!(parsed("B12 - Hall Of Mirrors", "x"), (s("B12"), "Hall Of Mirrors".into(), None), "a band called B12 before a title");
        assert_eq!(parsed("[Deleted video]", ""), (None, "[Deleted video]".into(), None), "an unavailable video keeps its marker");
    }

    #[test]
    fn non_music_is_left_out_and_duplicates_share_a_key() {
        let v = |title: &str, channel: &str, seconds: i64, playable: bool| Video { id: "x".into(), title: title.into(), channel: channel.into(), seconds: Some(seconds), playable };
        let check = |video: Video| {
            let p = parse_title(&video.title, &video.channel);
            leave_out(&video, &p)
        };
        assert_eq!(check(v("Kerri Chandler - Rain (Original Mix)", "x", 432, true)), None);
        assert_eq!(check(v("Pépé Bradock - Deep Burnt - Preview", "x", 95, true)), None, "short, but artist – title");
        assert!(check(v("Intro", "RecordDigger", 58, true)).unwrap().contains("Under 2 minutes"));
        assert!(check(v("Larry Heard - The Sun Can't Compare #shorts", "x", 47, true)).unwrap().contains("Short"));
        assert!(check(v("Deep House Mix 2026", "x", 10_801, true)).unwrap().contains("30 minutes"));
        assert!(check(v("[Deleted video]", "", 0, false)).unwrap().contains("no longer available"));
        assert_eq!(norm("Pépé Bradock & Co.  —  Deep Burnt!"), "pepe bradock and co deep burnt");
        let a = parse_title("Kerri Chandler - Rain (Original Mix)", "Kerri Chandler - Topic");
        let b = parse_title("Kerri Chandler – Rain (Original Mix) [Official Audio]", "deephousevault");
        let c = parse_title("Kerri Chandler - Rain (Dub Mix)", "x");
        assert_eq!(track_key(&a), track_key(&b));
        assert_ne!(track_key(&a), track_key(&c));
    }
}
