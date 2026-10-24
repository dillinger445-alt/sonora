//! Genre lookup on Discogs — the only feature that goes online, and only
//! when the user starts it for tracks they chose.
//!
//! What is sent: the artist and title of each track, the user's personal
//! access token and SONORA's name and version, over HTTPS to
//! api.discogs.com. Never audio, file names or paths. Requests go through the
//! system's `curl` (built into macOS and Windows 10/11), so SONORA itself
//! contains no network code. Discogs allows 60 requests a minute with a
//! token; SONORA stays below that.
//!
//! The result is the most common style among the matching releases ("Tech
//! House", "Deep House"…; the broad genre such as "Electronic" when no style
//! is given). Release genres and styles are CC0 data under the Discogs API
//! terms; the interface shows them with "Data provided by Discogs" and a link
//! to the release, as those terms require.

use crate::db::Db;
use crate::dj::DjConfig;
use crate::events::EventBus;
use crate::params;
use sonora_json::{obj, Value};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const API: &str = "https://api.discogs.com";
const SETTING_KEY: &str = "discogs_token";
/// Spacing between requests: 55 a minute at most.
const SPACING: Duration = Duration::from_millis(1100);
const TIMEOUT: Duration = Duration::from_secs(25);

#[derive(Clone, Debug, PartialEq)]
pub struct Response {
    pub status: u16,
    pub body: String,
}

/// How requests reach the network (replaced by a fake in tests).
pub trait Transport: Send + Sync {
    fn get(&self, url: &str, headers: &[(&str, String)]) -> Result<Response, String>;
}

/// HTTPS through the system's curl.
pub struct Curl {
    program: PathBuf,
}

impl Default for Curl {
    fn default() -> Self {
        let mut program = PathBuf::from("curl");
        if cfg!(windows) {
            if let Some(root) = std::env::var_os("SystemRoot") {
                let p = PathBuf::from(root).join("System32").join("curl.exe");
                if p.is_file() {
                    program = p;
                }
            }
        } else if PathBuf::from("/usr/bin/curl").is_file() {
            program = PathBuf::from("/usr/bin/curl");
        }
        Curl { program }
    }
}

/// Quotes a value for a curl config file.
fn config_value(v: &str) -> String {
    format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\"").replace(['\n', '\r'], " "))
}

impl Transport for Curl {
    fn get(&self, url: &str, headers: &[(&str, String)]) -> Result<Response, String> {
        // The configuration (and so the token) goes through stdin, not the
        // command line, where other programs on the computer could see it.
        let mut config = format!("url = {}\n", config_value(url));
        for (k, v) in headers {
            config += &format!("header = {}\n", config_value(&format!("{k}: {v}")));
        }
        config += "write-out = \"\\n%{http_code}\"\n";
        let mut cmd = sonora_engine::decode::tools::quiet_command(&self.program);
        // `-q` first: ignore the user's .curlrc, which could add headers,
        // cookies or tracing to requests that carry the token.
        cmd.args(["-q", "--silent", "--show-error", "--proto", "=https", "--tlsv1.2", "--max-time", "20", "--config", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| format!("curl could not be started ({e}); it is part of macOS and Windows 10 and later"))?;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(config.as_bytes());
        }
        let mut out = child.stdout.take().ok_or("no output from curl")?;
        let mut err = child.stderr.take().ok_or("no output from curl")?;
        let reader = std::thread::spawn(move || {
            let mut b = Vec::new();
            let _ = out.read_to_end(&mut b);
            b
        });
        let err_reader = std::thread::spawn(move || {
            let mut b = Vec::new();
            let _ = (&mut err).take(64 * 1024).read_to_end(&mut b);
            b
        });
        let started = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break s,
                Ok(None) if started.elapsed() > TIMEOUT => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("Discogs did not answer in time".into());
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                Err(e) => return Err(e.to_string()),
            }
        };
        let body = String::from_utf8_lossy(&reader.join().unwrap_or_default()).to_string();
        let err = String::from_utf8_lossy(&err_reader.join().unwrap_or_default()).trim().to_string();
        if !status.success() {
            // Only curl's own error line ("curl: (6) Could not resolve host…"),
            // never anything else it printed.
            let line = err.lines().rev().find(|l| l.starts_with("curl: (")).map(|l| l.trim_start_matches("curl: ").to_string());
            return Err(match line {
                Some(l) => format!("could not reach Discogs: {l}"),
                None => "the request to Discogs failed".into(),
            });
        }
        let (body, code) = body.rsplit_once('\n').unwrap_or(("", body.as_str()));
        let status = code.trim().parse::<u16>().map_err(|_| "unexpected answer from curl".to_string())?;
        Ok(Response { status, body: body.to_string() })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Match {
    /// The style (or genre) chosen for the track.
    pub genre: String,
    pub styles: Vec<String>,
    pub genres: Vec<String>,
    pub release: String,
    pub url: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LookupError {
    /// The token was refused (401/403).
    Unauthorized,
    /// Too many requests (429) even after waiting.
    RateLimited,
    Failed(String),
}

impl std::fmt::Display for LookupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LookupError::Unauthorized => f.write_str("Discogs did not accept the token. Check it in Settings → Genres."),
            LookupError::RateLimited => f.write_str("Discogs is limiting requests right now. Try again in a minute."),
            LookupError::Failed(m) => f.write_str(m),
        }
    }
}

fn percent(s: &str) -> String {
    let mut o = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => o.push(b as char),
            _ => o.push_str(&format!("%{b:02X}")),
        }
    }
    o
}

/// Removes mix descriptors that Discogs track titles usually leave out:
/// "(Original Mix)", "(Extended Mix)", "[Radio Edit]", "feat. X".
pub fn clean_title(title: &str) -> String {
    let mut t = title.to_string();
    for (open, close) in [('(', ')'), ('[', ']')] {
        while let (Some(a), Some(b)) = (t.find(open), t.find(close)) {
            if b <= a {
                break;
            }
            let inner = t[a + 1..b].to_ascii_lowercase();
            let drop = ["original mix", "extended mix", "radio edit", "club mix", "extended", "original", "edit", "dub mix", "feat", "ft."].iter().any(|w| inner.contains(w));
            if !drop {
                break;
            }
            t.replace_range(a..=b, " ");
        }
    }
    let lower = t.to_ascii_lowercase();
    for marker in [" feat. ", " feat ", " ft. ", " featuring "] {
        if let Some(i) = lower.find(marker) {
            t.truncate(i);
            break;
        }
    }
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A leading track number ("03 - ", "1. ", "07_", "03 "), and only that:
/// "808 State", "2 Unlimited" and "50 Cent" keep their names (B91).
fn strip_track_number(stem: &str) -> &str {
    let digits = stem.len() - stem.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    if digits == 0 || digits > 3 {
        return stem;
    }
    let after = &stem[digits..];
    let t = after.trim_start();
    if let Some(r) = t.strip_prefix(['-', '.', '_', ')']) {
        return r.trim_start_matches(['-', '.', '_', ' ']);
    }
    // A zero-padded number before a space is a track number too.
    if stem.starts_with('0') && after.starts_with(' ') {
        return t;
    }
    stem
}

/// "Artist - Title" from a file name, when tags are missing.
pub fn split_file_name(name: &str) -> Option<(String, String)> {
    let stem = std::path::Path::new(name).file_stem()?.to_string_lossy().to_string();
    let stem = strip_track_number(&stem);
    let (a, t) = stem.split_once(" - ")?;
    let (a, t) = (a.trim(), t.trim());
    (!a.is_empty() && !t.is_empty()).then(|| (a.to_string(), t.to_string()))
}

fn strings(v: &Value, k: &str) -> Vec<String> {
    v.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

/// Whether a release ("Artist - Title") is by `artist`: they share a word,
/// or it is a compilation ("Various").
fn same_artist(release_title: &str, artist: &str) -> bool {
    let release_artist = release_title.split(" - ").next().unwrap_or("").to_lowercase();
    if artist.trim().is_empty() || release_artist.starts_with("various") {
        return true;
    }
    let words = |s: &str| s.split(|c: char| !c.is_alphanumeric()).filter(|w| w.chars().count() >= 2).map(str::to_string).collect::<Vec<_>>();
    let ours = words(&artist.to_lowercase());
    let theirs = words(&release_artist);
    ours.iter().any(|w| theirs.contains(w))
}

/// Picks the style that the best-ranked matching releases agree on.
pub fn choose(results: &[Value], artist: &str) -> Option<Match> {
    let mut tally: Vec<(String, f64, usize)> = Vec::new();
    let mut considered = Vec::new();
    for (rank, r) in results.iter().take(10).enumerate() {
        let title = r.get("title").and_then(Value::as_str).unwrap_or("");
        if !same_artist(title, artist) {
            continue;
        }
        let styles = strings(r, "style");
        let genres = strings(r, "genre");
        let labels = if styles.is_empty() { genres.clone() } else { styles.clone() };
        let weight = 1.0 / (1.0 + rank as f64 * 0.5);
        for (i, s) in labels.iter().enumerate() {
            match tally.iter_mut().find(|t| t.0.eq_ignore_ascii_case(s)) {
                Some(t) => t.1 += weight,
                None => tally.push((s.clone(), weight, considered.len() * 100 + i)),
            }
        }
        considered.push((r, styles, genres));
    }
    tally.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then(a.2.cmp(&b.2)));
    let genre = tally.first()?.0.clone();
    let (r, styles, genres) = considered.iter().find(|(_, s, g)| s.iter().chain(g.iter()).any(|x| x.eq_ignore_ascii_case(&genre)))?;
    let uri = r.get("uri").and_then(Value::as_str).unwrap_or("");
    let url = if uri.starts_with("http") { uri.to_string() } else { format!("https://www.discogs.com{uri}") };
    Some(Match { genre, styles: styles.clone(), genres: genres.clone(), release: r.get("title").and_then(Value::as_str).unwrap_or("").to_string(), url })
}

pub struct Discogs {
    transport: Mutex<Arc<dyn Transport>>,
    user_agent: String,
    running: AtomicBool,
    cancel: AtomicBool,
    last: Mutex<Option<Value>>,
    last_request: Mutex<Option<Instant>>,
}

impl Discogs {
    pub fn new(app_version: &str) -> Discogs {
        Discogs {
            transport: Mutex::new(Arc::new(Curl::default())),
            user_agent: format!("SONORA/{app_version} +https://github.com/dillinger445-alt/sonora"),
            running: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            last: Mutex::new(None),
            last_request: Mutex::new(None),
        }
    }

    /// Replaces the network access (tests).
    pub fn set_transport(&self, t: Arc<dyn Transport>) {
        if let Ok(mut g) = self.transport.lock() {
            *g = t;
        }
    }

    pub fn token(db: &Db) -> Option<String> {
        db.read().query_row("SELECT value FROM settings WHERE key = ?", params![SETTING_KEY], |r| r.text(0)).ok().flatten().filter(|t| !t.is_empty())
    }

    /// Stores (or with `None` removes) the token. Tokens are letters and digits.
    pub fn set_token(db: &Db, token: Option<&str>) -> Result<(), String> {
        let c = db.write();
        match token.map(str::trim).filter(|t| !t.is_empty()) {
            Some(t) => {
                if t.len() > 100 || !t.chars().all(|c| c.is_ascii_alphanumeric()) {
                    return Err("a Discogs personal access token consists of letters and digits".into());
                }
                c.execute("INSERT INTO settings (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value", params![SETTING_KEY, t])
                    .map_err(|e| e.to_string())?;
            }
            None => {
                c.execute("DELETE FROM settings WHERE key = ?", params![SETTING_KEY]).map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    pub fn cancel(&self) -> bool {
        if self.is_running() {
            self.cancel.store(true, Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    pub fn last(&self) -> Value {
        self.last.lock().ok().and_then(|l| l.clone()).unwrap_or(Value::Null)
    }

    fn request(&self, token: &str, path_and_query: &str) -> Result<Value, LookupError> {
        let transport = self.transport.lock().map(|t| Arc::clone(&t)).map_err(|_| LookupError::Failed("internal error".into()))?;
        let headers = [("Authorization", format!("Discogs token={token}")), ("User-Agent", self.user_agent.clone()), ("Accept", "application/json".into())];
        let url = format!("{API}{path_and_query}");
        for attempt in 0..2 {
            // Keep requests spaced out, across calls.
            if let Ok(mut last) = self.last_request.lock() {
                if let Some(t) = *last {
                    let since = t.elapsed();
                    if since < SPACING {
                        std::thread::sleep(SPACING - since);
                    }
                }
                *last = Some(Instant::now());
            }
            let r = transport.get(&url, &headers).map_err(LookupError::Failed)?;
            match r.status {
                200 => return sonora_json::parse(&r.body).map_err(|_| LookupError::Failed("Discogs sent an answer SONORA could not read".into())),
                401 | 403 => return Err(LookupError::Unauthorized),
                429 if attempt == 0 => {
                    // Wait out the one-minute window, unless cancelled.
                    for _ in 0..60 {
                        if self.cancel.load(Ordering::Relaxed) {
                            return Err(LookupError::RateLimited);
                        }
                        std::thread::sleep(Duration::from_secs(1));
                    }
                }
                429 => return Err(LookupError::RateLimited),
                s => return Err(LookupError::Failed(format!("Discogs answered with error {s}"))),
            }
        }
        Err(LookupError::RateLimited)
    }

    /// Checks a token; returns the Discogs user name it belongs to.
    pub fn identity(&self, token: &str) -> Result<String, LookupError> {
        let v = self.request(token, "/oauth/identity")?;
        Ok(v.get("username").and_then(Value::as_str).unwrap_or("").to_string())
    }

    /// Finds the style of a track: by artist and track title, then with the
    /// mix descriptor removed, then as free text.
    pub fn search(&self, token: &str, artist: &str, title: &str) -> Result<Option<Match>, LookupError> {
        let cleaned = clean_title(title);
        let mut queries = vec![format!("/database/search?type=release&per_page=10&artist={}&track={}", percent(artist), percent(title))];
        if cleaned != title && !cleaned.is_empty() {
            queries.push(format!("/database/search?type=release&per_page=10&artist={}&track={}", percent(artist), percent(&cleaned)));
        }
        queries.push(format!("/database/search?type=release&per_page=10&q={}", percent(&format!("{artist} {cleaned}"))));
        for q in queries {
            if self.cancel.load(Ordering::Relaxed) {
                return Ok(None);
            }
            let v = self.request(token, &q)?;
            let results = v.get("results").and_then(Value::as_array).cloned().unwrap_or_default();
            if let Some(m) = choose(&results, artist) {
                return Ok(Some(m));
            }
        }
        Ok(None)
    }

    /// Looks up genres for `ids` in the background. `overwrite` also replaces
    /// genres the tracks already have. `false` if a lookup is already running.
    pub fn start(self: &Arc<Self>, db: Arc<Db>, events: Arc<EventBus>, ids: Vec<i64>, overwrite: bool, dj: DjConfig) -> bool {
        if self.running.swap(true, Ordering::SeqCst) {
            return false;
        }
        self.cancel.store(false, Ordering::SeqCst);
        let me = Arc::clone(self);
        let spawned = std::thread::Builder::new().name("sonora-discogs".into()).spawn(move || {
            let report = me.run(&db, &events, &ids, overwrite, &dj);
            if let Ok(mut l) = me.last.lock() {
                *l = Some(report.clone());
            }
            me.running.store(false, Ordering::SeqCst);
            events.emit("discogs.finished", &report);
            events.emit("library.changed", &obj! { "reason" => "discogs" });
        });
        if spawned.is_err() {
            self.running.store(false, Ordering::SeqCst);
            return false;
        }
        true
    }

    fn run(&self, db: &Db, events: &EventBus, ids: &[i64], overwrite: bool, dj: &DjConfig) -> Value {
        let Some(token) = Discogs::token(db) else {
            return obj! { "error" => "No Discogs token is set. Add yours in Settings → Genres." };
        };
        let tracks = crate::library::get_tracks(&db.read(), ids).unwrap_or_default();
        let total = tracks.len();
        let (mut found, mut not_found, mut skipped, mut failed) = (0i64, 0i64, 0i64, 0i64);
        let mut error: Option<String> = None;
        let mut cancelled = false;
        for (i, t) in tracks.iter().enumerate() {
            if self.cancel.load(Ordering::Relaxed) {
                cancelled = true;
                break;
            }
            events.emit("discogs.progress", &obj! { "done" => i, "total" => total, "found" => found, "current" => t.display_title() });
            // A genre taken from a folder name is only a fallback: look it up anyway.
            if !overwrite && t.dj_genre.is_some() && t.dj_genre_source.as_deref() != Some("folder") {
                skipped += 1;
                continue;
            }
            let pair = match (t.artist.as_deref().map(str::trim).filter(|a| !a.is_empty()), t.title.as_deref().map(str::trim).filter(|x| !x.is_empty())) {
                (Some(a), Some(x)) => Some((a.to_string(), x.to_string())),
                _ => split_file_name(&t.file_name),
            };
            let Some((artist, title)) = pair else {
                skipped += 1;
                continue;
            };
            match self.search(&token, &artist, &title) {
                Ok(Some(m)) => {
                    let stored = crate::dj::set_discogs_genre(&db.write(), t.id, &m.genre, &m.url, dj);
                    if stored.is_ok() {
                        found += 1;
                    } else {
                        failed += 1;
                    }
                }
                // Stopped part way (B90): a cancel, not a track not found or a
                // rate limit.
                Ok(None) | Err(LookupError::RateLimited) if self.cancel.load(Ordering::Relaxed) => {
                    cancelled = true;
                    break;
                }
                Ok(None) => not_found += 1,
                Err(e @ (LookupError::Unauthorized | LookupError::RateLimited)) => {
                    error = Some(e.to_string());
                    break;
                }
                Err(LookupError::Failed(m)) => {
                    failed += 1;
                    crate::logging::log_fields("warn", "discogs", &m, obj! { "track" => t.id });
                    if failed >= 3 && found == 0 {
                        // Probably offline: stop instead of failing every track.
                        error = Some(m);
                        break;
                    }
                }
            }
        }
        obj! {
            "total" => total, "found" => found, "notFound" => not_found, "skipped" => skipped, "failed" => failed,
            "cancelled" => cancelled, "error" => error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_and_file_names() {
        assert_eq!(clean_title("Mercury (Original Mix)"), "Mercury");
        assert_eq!(clean_title("Mercury (Extended Mix) [feat. Ana]"), "Mercury");
        assert_eq!(clean_title("Mercury (Dixon Remix)"), "Mercury (Dixon Remix)");
        assert_eq!(clean_title("Mercury feat. Ana"), "Mercury");
        assert_eq!(split_file_name("03 - Ana Ruiz - Night Drive (Original Mix).flac"), Some(("Ana Ruiz".into(), "Night Drive (Original Mix)".into())));
        assert_eq!(split_file_name("Night Drive.wav"), None);
        // B91: digits that are part of the artist's name stay.
        assert_eq!(split_file_name("808 State - Pacific.flac"), Some(("808 State".into(), "Pacific".into())));
        assert_eq!(split_file_name("2 Unlimited - Get Ready.mp3"), Some(("2 Unlimited".into(), "Get Ready".into())));
        assert_eq!(split_file_name("50 Cent - In Da Club.mp3"), Some(("50 Cent".into(), "In Da Club".into())));
        assert_eq!(split_file_name("1. Ana Ruiz - Night Drive.flac"), Some(("Ana Ruiz".into(), "Night Drive".into())));
        assert_eq!(split_file_name("07_Ana Ruiz - Night Drive.flac"), Some(("Ana Ruiz".into(), "Night Drive".into())));
        assert_eq!(split_file_name("03 Ana Ruiz - Night Drive.flac"), Some(("Ana Ruiz".into(), "Night Drive".into())));
        assert_eq!(percent("Ana & Bo/1"), "Ana%20%26%20Bo%2F1");
        assert_eq!(config_value("a \"b\" \\c"), "\"a \\\"b\\\" \\\\c\"");
    }

    #[test]
    fn the_style_most_releases_agree_on_wins() {
        let r = |title: &str, style: &[&str], genre: &[&str], uri: &str| {
            obj! { "title" => title, "style" => style.iter().map(|s| s.to_string()).collect::<Vec<_>>(), "genre" => genre.iter().map(|s| s.to_string()).collect::<Vec<_>>(), "uri" => uri }
        };
        let results = vec![
            r("Ana Ruiz - Night Drive EP", &["Deep House", "Tech House"], &["Electronic"], "/release/1-Night-Drive"),
            r("Ana Ruiz - Summer Comp", &["Tech House"], &["Electronic"], "/release/2"),
            r("Someone Else - Night Drive", &["Trance"], &["Electronic"], "/release/3"),
            r("Various - Summer Sampler", &["Tech House", "Minimal"], &["Electronic"], "/release/4"),
        ];
        let m = choose(&results, "Ana Ruiz").unwrap();
        assert_eq!(m.genre, "Tech House");
        assert_eq!(m.url, "https://www.discogs.com/release/1-Night-Drive");
        // Without styles the broad genre is used.
        let m = choose(&[r("Ana Ruiz - Single", &[], &["Electronic"], "/release/9")], "Ana Ruiz").unwrap();
        assert_eq!(m.genre, "Electronic");
        assert!(choose(&[r("Other - Thing", &["Techno"], &[], "/r")], "Ana Ruiz").is_none());
        assert!(choose(&[], "Ana Ruiz").is_none());
    }
}
