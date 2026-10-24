//! Downloading a YouTube playlist from Soulseek, against stand-ins for
//! YouTube (the saved playlist page and its continuation) and for slskd
//! (searches answered by what they ask for, downloads "received" into the
//! download folder). Nothing reaches the network.

use sonora_core::net::{Http, NetError, Request, Response};
use sonora_core::{App, AppConfig};
use sonora_engine::decode::wav::write_wav;
use sonora_engine::synth::{quantize16, render, SynthOptions};
use sonora_json::{obj, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

const KEY: &str = "playlist-key-0123456789abc";
const PLAYLIST: &str = include_str!("fixtures/youtube/playlist.html");
const CONTINUATION: &str = include_str!("fixtures/youtube/continuation.json");

static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

// ------------------------------------------------------------ YouTube

struct FakeTube;

impl Http for FakeTube {
    fn send(&self, req: &Request) -> Result<Response, NetError> {
        if req.url.starts_with("https://tube.test/playlist?list=PLfixtureLateNight") {
            return Ok(Response { status: 200, body: PLAYLIST.into() });
        }
        if req.url.starts_with("https://tube.test/youtubei/v1/browse") {
            return Ok(Response { status: 200, body: CONTINUATION.into() });
        }
        Ok(Response { status: 404, body: String::new() })
    }
}

// -------------------------------------------------------------- slskd

#[derive(Clone, Debug)]
struct Transfer {
    id: String,
    username: String,
    filename: String,
    size: i64,
    state: String,
    bytes: i64,
}

#[derive(Default)]
struct World {
    searches: HashMap<String, (String, usize)>,
    /// Every search text asked, in order.
    asked: Vec<String>,
    transfers: Vec<Transfer>,
    next: u64,
    /// Searches to refuse (slskd restarting), before answering again.
    refuse: usize,
}

struct FakeSlskd {
    world: Mutex<World>,
    downloads: PathBuf,
}

fn guid(n: u64) -> String {
    format!("00000000-0000-4000-8000-{n:012x}")
}

fn result_file(name: &str, ext: &str, size: i64, depth: i64, rate: i64, length: i64) -> Value {
    obj! { "filename" => format!("@@abcde\\Music\\Album\\{name}.{ext}"), "size" => size, "bitDepth" => depth, "sampleRate" => rate, "length" => length, "extension" => "" }
}

/// A file in a folder named after the artist (the file's own name is not).
fn in_folder(folder: &str, name: &str, ext: &str, size: i64, depth: i64, rate: i64, length: i64) -> Value {
    obj! { "filename" => format!("@@abcde\\Music\\{folder}\\{name}.{ext}"), "size" => size, "bitDepth" => depth, "sampleRate" => rate, "length" => length, "extension" => "" }
}

/// Who answers a search, with what: made up to exercise the rules.
fn answers(text: &str) -> Vec<Value> {
    let t = text.to_lowercase();
    let good = |files: Vec<Value>| obj! { "username" => "good", "hasFreeUploadSlot" => true, "queueLength" => 0, "uploadSpeed" => 1_500_000, "files" => Value::Array(files) };
    let mut out = Vec::new();
    if t.contains("rain") {
        out.push(good(vec![in_folder("Kerri Chandler - Rain EP", "01 Rain (Original Mix)", "flac", 40_000_000, 16, 44_100, 432)]));
        out.push(obj! { "username" => "mp3er", "hasFreeUploadSlot" => true, "queueLength" => 0, "uploadSpeed" => 900_000, "files" => Value::Array(vec![obj! { "filename" => "@@x\\Rain (Original Mix).mp3", "size" => 10_000_000, "bitRate" => 320, "length" => 432 }]) });
    } else if t.contains("blackwater") {
        if t.contains("octave") {
            out.push(good(vec![result_file("Octave One - Blackwater", "flac", 38_000_000, 16, 44_100, 401)]));
        } else {
            out.push(obj! { "username" => "remixer", "hasFreeUploadSlot" => true, "queueLength" => 0, "uploadSpeed" => 900_000, "files" => Value::Array(vec![result_file("Blackwater (Dub Remix)", "flac", 38_000_000, 16, 44_100, 500)]) });
        }
    } else if t.contains("falling") {
        // The faster person's file fails to download; the other one works.
        out.push(obj! { "username" => "bad", "hasFreeUploadSlot" => true, "queueLength" => 0, "uploadSpeed" => 9_000_000, "files" => Value::Array(vec![result_file("Theo Parrish - Falling Up (Carl Craig Remix)", "flac", 50_000_000, 16, 44_100, 538)]) });
        out.push(good(vec![result_file("Theo Parrish - Falling Up (Carl Craig Remix)", "flac", 50_000_000, 16, 44_100, 538)]));
    } else if t.contains("don't go") || t.contains("dont go") {
        out.push(good(vec![result_file("Floorplan - Don't Go (Extended Mix)", "flac", 34_000_000, 24, 44_100, 366)]));
    } else if t.contains("burnt") {
        out.push(good(vec![result_file("Pépé Bradock - Deep Burnt", "flac", 9_000_000, 16, 44_100, 95)]));
    } else if t.contains("morning") {
        out.push(good(vec![result_file("Ron Trent - Morning Factory", "aiff", 108_000_000, 16, 44_100, 611)]));
    } else if t.contains("night driving") {
        out.push(good(vec![result_file("Rick Wade - Night Driving (Radio Edit)", "flac", 27_000_000, 16, 44_100, 288)]));
    }
    out
}

impl FakeSlskd {
    /// "Receives" a download: a short WAV lands in the download folder (as
    /// an older file, so the watcher takes it at once), unless the sharer
    /// is "bad".
    fn receive(&self, id: &str) {
        let (username, filename) = {
            let w = self.world.lock().unwrap();
            let t = w.transfers.iter().find(|t| t.id == id).unwrap();
            (t.username.clone(), t.filename.clone())
        };
        std::thread::sleep(Duration::from_millis(150));
        let mut w = self.world.lock().unwrap();
        let t = w.transfers.iter_mut().find(|t| t.id == id).unwrap();
        if t.state.starts_with("Completed") {
            return;
        }
        if username == "bad" {
            t.state = "Completed, Errored".into();
            return;
        }
        let name = filename.rsplit('\\').next().unwrap().to_string();
        let path = self.downloads.join(&username).join(&name);
        let size = write_download(&path);
        t.bytes = size;
        t.size = size;
        t.state = "Completed, Succeeded".into();
    }

    fn transfer_json(t: &Transfer) -> Value {
        obj! {
            "id" => t.id.clone(), "username" => t.username.clone(), "filename" => t.filename.clone(), "size" => t.size, "state" => t.state.clone(),
            "bytesTransferred" => t.bytes, "placeInQueue" => Value::Null, "requestedAt" => "2026-10-21T01:00:00Z",
        }
    }
}

impl Http for FakeSlskd {
    fn send(&self, req: &Request) -> Result<Response, NetError> {
        let path = req.url.strip_prefix("http://localhost:5030/api/v0/").expect("slskd's address").to_string();
        if req.headers.iter().find(|(k, _)| k == "X-API-Key").map(|(_, v)| v.as_str()) != Some(KEY) {
            return Ok(Response { status: 401, body: String::new() });
        }
        let ok = |v: Value| Ok(Response { status: 200, body: v.to_string_compact() });
        let (route, query) = path.split_once('?').unwrap_or((path.as_str(), ""));
        let parts: Vec<String> = route.split('/').map(|p| p.replace("%20", " ")).collect();
        let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
        match (req.method, parts.as_slice()) {
            ("GET", ["application"]) => ok(obj! {
                "version" => obj! { "current" => "0.26.1" },
                "server" => obj! { "state" => "Connected, LoggedIn", "isConnected" => true, "isLoggedIn" => true },
                "user" => obj! { "username" => "me" },
                "shares" => obj! { "directories" => 1, "files" => 10 },
            }),
            ("POST", ["searches"]) => {
                {
                    let mut w = self.world.lock().unwrap();
                    if w.refuse > 0 {
                        w.refuse -= 1;
                        return Ok(Response { status: 503, body: String::new() });
                    }
                }
                let text = sonora_json::parse(req.body.as_deref().unwrap_or("")).ok().and_then(|b| b.get("searchText").and_then(Value::as_str).map(str::to_string)).unwrap_or_default();
                let mut w = self.world.lock().unwrap();
                w.next += 1;
                let id = guid(w.next);
                w.searches.insert(id.clone(), (text.clone(), 0));
                w.asked.push(text.clone());
                ok(obj! { "id" => id, "searchText" => text, "state" => "InProgress" })
            }
            ("GET", ["searches", id]) => {
                let mut w = self.world.lock().unwrap();
                let Some((text, looks)) = w.searches.get_mut(*id) else { return Ok(Response { status: 404, body: String::new() }) };
                *looks += 1;
                let responses = answers(text);
                let mut v = obj! { "id" => *id, "searchText" => text.clone(), "responseCount" => responses.len() as i64 };
                match *looks {
                    1 => v.set("state", "InProgress"),
                    2 => v.set("state", "Completed, TimedOut"),
                    _ => {
                        v.set("state", "Completed, TimedOut");
                        v.set("endedAt", "2026-10-21T01:00:15Z");
                        if query.contains("includeResponses=true") {
                            v.set("responses", Value::Array(responses));
                        }
                    }
                }
                ok(v)
            }
            ("PUT", ["searches", _]) => Ok(Response { status: 200, body: String::new() }),
            ("POST", ["transfers", "downloads", user]) => {
                let files = sonora_json::parse(req.body.as_deref().unwrap_or("[]")).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
                let mut ids = Vec::new();
                {
                    let mut w = self.world.lock().unwrap();
                    for f in &files {
                        w.next += 1;
                        let id = guid(w.next);
                        w.transfers.push(Transfer {
                            id: id.clone(),
                            username: user.to_string(),
                            filename: f.get("filename").and_then(Value::as_str).unwrap_or("").to_string(),
                            size: f.get("size").and_then(Value::as_i64).unwrap_or(0),
                            state: "Queued, Remotely".into(),
                            bytes: 0,
                        });
                        ids.push(id);
                    }
                }
                let enqueued: Vec<Value> = {
                    let w = self.world.lock().unwrap();
                    w.transfers.iter().filter(|t| ids.contains(&t.id)).map(FakeSlskd::transfer_json).collect()
                };
                for id in ids {
                    self.receive(&id);
                }
                Ok(Response { status: 201, body: obj! { "enqueued" => Value::Array(enqueued), "failed" => Value::Array(vec![]) }.to_string_compact() })
            }
            ("GET", ["transfers", "downloads", user, id]) => {
                let w = self.world.lock().unwrap();
                match w.transfers.iter().find(|t| t.id == *id && t.username == *user) {
                    Some(t) => ok(FakeSlskd::transfer_json(t)),
                    None => Ok(Response { status: 404, body: String::new() }),
                }
            }
            ("DELETE", ["transfers", "downloads", _, id]) => {
                let mut w = self.world.lock().unwrap();
                if let Some(t) = w.transfers.iter_mut().find(|t| t.id == *id) {
                    if !t.state.starts_with("Completed") {
                        t.state = "Completed, Cancelled".into();
                    }
                }
                Ok(Response { status: 204, body: String::new() })
            }
            ("GET", ["transfers", "downloads", user]) => {
                let w = self.world.lock().unwrap();
                let mine: Vec<Value> = w.transfers.iter().filter(|t| t.username == *user).map(FakeSlskd::transfer_json).collect();
                if mine.is_empty() {
                    return Ok(Response { status: 404, body: String::new() });
                }
                ok(obj! { "username" => *user, "directories" => Value::Array(vec![obj! { "directory" => "x", "files" => Value::Array(mine) }]) })
            }
            ("GET", ["transfers", "downloads"]) => {
                let w = self.world.lock().unwrap();
                let users: Vec<String> = w.transfers.iter().map(|t| t.username.clone()).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
                ok(Value::Array(
                    users
                        .iter()
                        .map(|u| obj! { "username" => u.clone(), "directories" => Value::Array(vec![obj! { "directory" => "x", "files" => Value::Array(w.transfers.iter().filter(|t| &t.username == u).map(FakeSlskd::transfer_json).collect()) }]) })
                        .collect(),
                ))
            }
            _ => Ok(Response { status: 404, body: String::new() }),
        }
    }
}

/// A short WAV, dated two minutes ago so the watcher takes it at once.
fn write_download(p: &Path) -> i64 {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    let samples = quantize16(&render(&SynthOptions { seconds: 4.0, seed: 11, variant: 3, ..Default::default() }));
    write_wav(p, 44_100, 2, 16, false, &samples).unwrap();
    std::fs::File::options().write(true).open(p).unwrap().set_modified(SystemTime::now() - Duration::from_secs(120)).unwrap();
    std::fs::metadata(p).unwrap().len() as i64
}

// ------------------------------------------------------------- helpers

struct Env {
    root: PathBuf,
    downloads: PathBuf,
    data: PathBuf,
}

fn env(name: &str) -> Env {
    // The folder watcher looks only when asked (an hour is never reached
    // here): downloads must arrive through the run's own requests to look,
    // as they do in the app between its 20-second looks (B01).
    std::env::set_var("SONORA_SOURCE_POLL_SECONDS", "3600");
    std::env::set_var("SONORA_SECRET_STORE", "file");
    std::env::set_var("SONORA_YOUTUBE_BASE", "https://tube.test");
    let root = std::env::temp_dir().join(format!("sonora-playlist-it-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let downloads = root.join("downloads");
    let data = root.join("data");
    std::fs::create_dir_all(&downloads).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    Env { root, downloads, data }
}

fn open(e: &Env) -> (Arc<App>, Arc<FakeSlskd>) {
    let app = App::open(AppConfig { data_dir: e.data.clone(), tool_dirs: vec![], app_version: "test".into() }).unwrap();
    let fake = Arc::new(FakeSlskd { world: Mutex::new(World::default()), downloads: e.downloads.clone() });
    app.slskd.set_http(fake.clone());
    app.playlist.set_http(Arc::new(FakeTube));
    (app, fake)
}

fn call(app: &Arc<App>, method: &str, params: &Value) -> Value {
    let out = app.call(method, &params.to_string_compact()).unwrap_or_else(|e| panic!("{method}: {e}"));
    sonora_json::parse(&out).unwrap()
}

fn call_err(app: &Arc<App>, method: &str, params: &Value) -> (String, String) {
    let e = app.call(method, &params.to_string_compact()).expect_err(method);
    (e.code, e.message)
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

fn count(run: &Value, k: &str) -> i64 {
    run.get("counts").and_then(|c| c.get(k)).and_then(Value::as_i64).unwrap_or(-1)
}

fn wait_state(app: &Arc<App>, id: i64, states: &[&str], secs: u64) -> Value {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        let run = call(app, "playlist.get", &obj! { "id" => id });
        if states.contains(&s(&run, "state")) {
            return run;
        }
        assert!(Instant::now() < deadline, "run did not reach {states:?}: {} ({:?})", s(&run, "state"), run.get("counts"));
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn setup(app: &Arc<App>, e: &Env) {
    call(app, "sources.accept", &obj! {});
    call(app, "slskd.configure", &obj! { "url" => "http://localhost:5030", "apiKey" => KEY });
    call(app, "sources.configure", &obj! { "folder" => e.downloads.to_string_lossy().to_string() });
}

fn items_of(run: &Value) -> Vec<&Value> {
    run.get("items").and_then(Value::as_array).map(|a| a.iter().collect()).unwrap_or_default()
}

fn item<'a>(run: &'a Value, title: &str) -> &'a Value {
    items_of(run).into_iter().find(|i| s(i, "title") == title).unwrap_or_else(|| panic!("no item {title}"))
}

// --------------------------------------------------------------- tests

#[test]
fn a_playlist_is_read_searched_downloaded_and_checked() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let e = env("flow");
    let (app, fake) = open(&e);
    assert_eq!(call_err(&app, "playlist.fetch", &obj! { "source" => "PLfixtureLateNight" }).0, "not_accepted");
    setup(&app, &e);

    // Links are checked before anything is fetched.
    let (code, msg) = call_err(&app, "playlist.fetch", &obj! { "source" => "https://www.youtube.com/playlist?list=WL" });
    assert_eq!(code, "invalid_params");
    assert!(msg.contains("Watch Later"), "{msg}");

    // The list: read in the background, then a draft with its tracks.
    let id = call(&app, "playlist.fetch", &obj! { "source" => "https://www.youtube.com/playlist?list=PLfixtureLateNight" }).get("id").and_then(Value::as_i64).unwrap();
    let run = wait_state(&app, id, &["draft", "failed"], 20);
    assert_eq!(s(&run, "state"), "draft", "{:?}", run.get("error"));
    assert_eq!(s(&run, "title"), "Late Night Selections");
    assert_eq!(s(&run, "playlistId"), "PLfixtureLateNight");
    assert_eq!(count(&run, "total"), 13);
    assert_eq!(count(&run, "included"), 8);
    assert_eq!(count(&run, "leftOut"), 5);
    let left: Vec<(&str, &str)> = items_of(&run).into_iter().filter(|i| i.get("included") == Some(&Value::Bool(false))).map(|i| (s(i, "ytTitle"), s(i, "leftOut"))).collect();
    assert_eq!(left.len(), 5, "{left:?}");
    assert!(left.iter().any(|(t, why)| *t == "Intro" && why.contains("Under 2 minutes")), "{left:?}");
    assert!(left.iter().any(|(t, why)| *t == "[Deleted video]" && why.contains("no longer available")), "{left:?}");
    assert!(left.iter().any(|(t, why)| t.contains("#shorts") && why.contains("Short")), "{left:?}");
    assert!(left.iter().any(|(t, why)| t.starts_with("Deep House Mix") && why.contains("30 minutes")), "{left:?}");
    assert!(left.iter().any(|(t, why)| t.starts_with("Kerri Chandler - Rain") && why.contains("already")), "the second Rain: {left:?}");
    let rain = item(&run, "Rain");
    assert_eq!((s(rain, "artist"), s(rain, "mix"), s(rain, "status"), s(rain, "url")), ("Kerri Chandler", "Original Mix", "pending", "https://www.youtube.com/watch?v=a1b2c3d4e5F"));
    let wade = item(&run, "Night Driving");
    assert_eq!((s(wade, "artist"), s(wade, "mix"), s(wade, "catalogue")), ("Rick Wade", "Radio Edit", "HG007"));

    // A left-out video can be put back, and a wanted one taken out.
    let intro_id = item(&run, "Intro").get("id").and_then(Value::as_i64).unwrap();
    let run = call(&app, "playlist.include", &obj! { "id" => id, "itemIds" => Value::Array(vec![Value::Int(intro_id)]), "included" => true });
    assert_eq!(count(&run, "included"), 9);
    let run = call(&app, "playlist.include", &obj! { "id" => id, "itemIds" => Value::Array(vec![Value::Int(intro_id)]), "included" => false });
    assert_eq!(count(&run, "included"), 8);

    // The rules in use are kept with the run.
    call(&app, "settings.update", &obj! { "values" => obj! { "playlistRules" => obj! { "maxDownloads" => 2, "searchSeconds" => 15, "retryNotFound" => false } } });
    let run = call(&app, "playlist.start", &obj! { "id" => id });
    assert_eq!(s(&run, "state"), "running");
    assert_eq!(run.get("rules").and_then(|r| r.get("maxDownloads")).and_then(Value::as_i64), Some(2));
    assert_eq!(call_err(&app, "playlist.start", &obj! { "id" => id }).0, "invalid_params", "already running");
    assert_eq!(call_err(&app, "playlist.delete", &obj! { "id" => id }).0, "busy");

    // Pause holds the search between tracks; resume goes on.
    let paused = call(&app, "playlist.pause", &obj! { "id" => id });
    assert_eq!(s(&paused, "state"), "paused");
    std::thread::sleep(Duration::from_millis(300));
    // Rules changed while paused apply from then on (B16).
    call(&app, "settings.update", &obj! { "values" => obj! { "playlistRules" => obj! { "maxDownloads" => 3, "searchSeconds" => 15, "retryNotFound" => false } } });
    let run = call(&app, "playlist.resume", &obj! { "id" => id });
    assert_eq!(s(&run, "state"), "running");
    assert_eq!(run.get("rules").and_then(|r| r.get("maxDownloads")).and_then(Value::as_i64), Some(3), "resumed with the rules in use now");

    let run = wait_state(&app, id, &["finished", "cancelled", "failed", "review", "waiting"], 180);
    assert_eq!(s(&run, "state"), "finished", "{:?} {:?}", run.get("error"), run.get("counts"));
    assert_eq!(count(&run, "done"), 7, "{:?}", items_of(&run).iter().map(|i| (s(i, "title"), s(i, "status"), s(i, "error"))).collect::<Vec<_>>());
    assert_eq!(count(&run, "notFound"), 1);
    assert_eq!(count(&run, "failed"), 0);

    // What each track went through.
    let rain = item(&run, "Rain");
    assert_eq!((s(rain, "status"), rain.get("variation").and_then(Value::as_i64)), ("done", Some(1)));
    let cands = rain.get("candidates").and_then(Value::as_array).unwrap();
    assert_eq!(cands.len(), 2);
    assert_eq!((s(&cands[0], "username"), s(&cands[0], "ext"), cands[0].get("rejected")), ("good", "flac", Some(&Value::Null)));
    assert!(s(&cands[1], "rejected").contains("lossy formats are off"), "{cands:?}");
    assert_eq!(rain.get("chosen").and_then(Value::as_i64), Some(0));
    let track = rain.get("track").unwrap();
    assert_eq!(s(track, "status"), "done", "the arrived file was analysed: {track:?}");
    assert!(s(track, "path").ends_with("01 Rain (Original Mix).flac"));

    let octave = item(&run, "Blackwater");
    assert_eq!((s(octave, "status"), octave.get("variation").and_then(Value::as_i64)), ("done", Some(2)), "the title alone found only a remix: {octave:?}");

    let shades = item(&run, "Shades Of Jae");
    assert_eq!(s(shades, "status"), "not_found");
    assert!(s(shades, "error").contains("No one answered"), "{shades:?}");

    let falling = item(&run, "Falling Up");
    assert_eq!(s(falling, "status"), "done");
    assert_eq!(falling.get("chosen").and_then(Value::as_i64), Some(1), "the second candidate, after the first failed");
    assert_eq!(falling.get("attempts").and_then(Value::as_i64), Some(2));
    assert!(s(falling, "note").contains("did not work out") && s(falling, "note").contains("trying the next"), "{:?}", falling.get("note"));

    let wade = item(&run, "Night Driving");
    assert_eq!(s(wade, "status"), "done", "a radio edit is wanted, so the name may say so: {wade:?}");

    // Every search went through slskd, fewest words first, and the
    // not-found track was asked for with each of its variations.
    let asked = fake.world.lock().unwrap().asked.clone();
    assert_eq!(asked[0], "Rain Original Mix", "{asked:?}");
    assert!(asked.contains(&"Blackwater".to_string()) && asked.contains(&"Octave One Blackwater".to_string()), "{asked:?}");
    assert!(asked.contains(&"Shades Of Jae".to_string()) && asked.contains(&"Moodymann Shades Of Jae".to_string()), "{asked:?}");
    assert!(!asked.iter().any(|a| a.contains('(')), "no punctuation goes to Soulseek: {asked:?}");

    // The files are in the library, from Soulseek.
    let n = app.db.read().query_row("SELECT COUNT(*) FROM tracks", sonora_core::params![], |r| r.i64(0)).unwrap().unwrap();
    assert_eq!(n, 7);

    // Looking again for what was not found starts the run once more.
    let run = call(&app, "playlist.retry", &obj! { "id" => id });
    assert_eq!(s(&run, "state"), "running");
    let run = wait_state(&app, id, &["finished"], 60);
    assert_eq!(count(&run, "notFound"), 1);
    assert_eq!(count(&run, "done"), 7);

    // The run is listed, and can be removed once it is over.
    let list = call(&app, "playlist.list", &obj! {});
    assert_eq!(list.get("runs").and_then(Value::as_array).map(Vec::len), Some(1));
    call(&app, "playlist.delete", &obj! { "id" => id });
    assert_eq!(call_err(&app, "playlist.get", &obj! { "id" => id }).0, "not_found");
    app.shutdown();
    let _ = std::fs::remove_dir_all(&e.root);
}

#[test]
fn review_mode_waits_for_the_picks_and_a_run_survives_a_restart() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let e = env("review");
    let (app, _fake) = open(&e);
    setup(&app, &e);
    let id = call(&app, "playlist.fetch", &obj! { "source" => "PLfixtureLateNight" }).get("id").and_then(Value::as_i64).unwrap();
    wait_state(&app, id, &["draft"], 20);
    // Only two tracks, to keep this short.
    let run = call(&app, "playlist.get", &obj! { "id" => id });
    let keep = ["Rain", "Falling Up"];
    let out: Vec<Value> = items_of(&run).iter().filter(|i| i.get("included") == Some(&Value::Bool(true)) && !keep.contains(&s(i, "title"))).map(|i| Value::Int(i.get("id").and_then(Value::as_i64).unwrap())).collect();
    call(&app, "playlist.include", &obj! { "id" => id, "itemIds" => Value::Array(out), "included" => false });
    call(&app, "settings.update", &obj! { "values" => obj! { "playlistRules" => obj! { "mode" => "review", "searchSeconds" => 15, "retryNotFound" => false } } });
    call(&app, "playlist.start", &obj! { "id" => id });
    let run = wait_state(&app, id, &["review", "finished", "failed"], 90);
    assert_eq!(s(&run, "state"), "review", "{:?}", run.get("counts"));
    assert_eq!(count(&run, "found"), 2);
    assert_eq!(count(&run, "downloading"), 0, "nothing is downloaded before the picks are confirmed");

    // Another pick for Falling Up: the slower person, whose file works.
    let falling = item(&run, "Falling Up");
    let falling_id = falling.get("id").and_then(Value::as_i64).unwrap();
    assert_eq!(s(&falling.get("candidates").unwrap().as_array().unwrap()[0], "username"), "bad");
    let (code, _) = call_err(&app, "playlist.choose", &obj! { "id" => id, "itemId" => falling_id, "candidate" => 5 });
    assert_eq!(code, "invalid_params");
    call(&app, "playlist.choose", &obj! { "id" => id, "itemId" => falling_id, "candidate" => 1 });
    let run = call(&app, "playlist.get", &obj! { "id" => id });
    assert_eq!(item(&run, "Falling Up").get("chosen").and_then(Value::as_i64), Some(1));

    // Quit in the middle: the run carries on when SONORA opens again.
    call(&app, "playlist.confirm", &obj! { "id" => id });
    app.shutdown();
    drop(app);
    let (app, _fake) = open(&e);
    let run = wait_state(&app, id, &["finished", "failed", "cancelled"], 120);
    assert_eq!(s(&run, "state"), "finished", "{:?}", run.get("counts"));
    assert_eq!(count(&run, "done"), 2, "{:?}", items_of(&run).iter().map(|i| (s(i, "title"), s(i, "status"), s(i, "note"), s(i, "error"))).collect::<Vec<_>>());
    let falling = item(&run, "Falling Up");
    assert_eq!(falling.get("attempts").and_then(Value::as_i64), Some(1), "the person's own pick worked first time");
    assert!(s(falling, "note").starts_with("You chose"), "{:?}", falling.get("note"));
    app.shutdown();
    let _ = std::fs::remove_dir_all(&e.root);
}

#[test]
fn a_run_can_be_stopped() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let e = env("stop");
    let (app, _fake) = open(&e);
    setup(&app, &e);
    let id = call(&app, "playlist.fetch", &obj! { "source" => "PLfixtureLateNight" }).get("id").and_then(Value::as_i64).unwrap();
    wait_state(&app, id, &["draft"], 20);
    call(&app, "settings.update", &obj! { "values" => obj! { "playlistRules" => obj! { "searchSeconds" => 15 } } });
    call(&app, "playlist.start", &obj! { "id" => id });
    std::thread::sleep(Duration::from_millis(600));
    let run = call(&app, "playlist.cancel", &obj! { "id" => id });
    assert!(matches!(s(&run, "state"), "running" | "cancelled"));
    let run = wait_state(&app, id, &["cancelled"], 30);
    assert_eq!(count(&run, "searching"), 0, "{:?}", run.get("counts"));
    assert!(app.playlist.is_active().is_none() || app.playlist.is_active() != Some(id));
    // Stopped runs can be started again.
    let run = call(&app, "playlist.start", &obj! { "id" => id });
    assert_eq!(s(&run, "state"), "running");
    call(&app, "playlist.cancel", &obj! { "id" => id });
    wait_state(&app, id, &["cancelled"], 30);
    app.shutdown();
    let _ = std::fs::remove_dir_all(&e.root);
}

/// Keeps only the named tracks of a fetched run.
fn only(app: &Arc<App>, id: i64, keep: &[&str]) {
    let run = call(app, "playlist.get", &obj! { "id" => id });
    let out: Vec<Value> = items_of(&run).iter().filter(|i| i.get("included") == Some(&Value::Bool(true)) && !keep.contains(&s(i, "title"))).map(|i| Value::Int(i.get("id").and_then(Value::as_i64).unwrap())).collect();
    call(app, "playlist.include", &obj! { "id" => id, "itemIds" => Value::Array(out), "included" => false });
}

fn wait_for(what: &str, secs: u64, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn one_run_works_at_a_time_and_the_others_are_left_as_they_were() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let e = env("turns");
    let (app, fake) = open(&e);
    setup(&app, &e);
    let a = call(&app, "playlist.fetch", &obj! { "source" => "PLfixtureLateNight" }).get("id").and_then(Value::as_i64).unwrap();
    wait_state(&app, a, &["draft"], 20);
    let b = call(&app, "playlist.fetch", &obj! { "source" => "PLfixtureLateNight" }).get("id").and_then(Value::as_i64).unwrap();
    wait_state(&app, b, &["draft"], 20);
    // A looks for the track nobody has, and will look again later.
    only(&app, a, &["Shades Of Jae"]);
    call(&app, "settings.update", &obj! { "values" => obj! { "playlistRules" => obj! { "searchSeconds" => 15, "retryNotFound" => true } } });
    call(&app, "playlist.start", &obj! { "id" => a });

    // B03: B is refused and stays exactly as it was.
    let before = call(&app, "playlist.get", &obj! { "id" => b });
    assert_eq!(call_err(&app, "playlist.start", &obj! { "id" => b }).0, "busy");
    assert_eq!(call_err(&app, "playlist.retry", &obj! { "id" => b }).0, "busy");
    let after = call(&app, "playlist.get", &obj! { "id" => b });
    assert_eq!(s(&after, "state"), "draft");
    assert_eq!(after.get("rules"), before.get("rules"), "the rules were not written");
    assert_eq!(after.get("counts"), before.get("counts"));

    // A waits to look again; its worker is still on it, so B still waits.
    wait_state(&app, a, &["waiting"], 120);
    assert_eq!(call_err(&app, "playlist.start", &obj! { "id" => b }).0, "busy");
    assert_eq!(s(&call(&app, "playlist.get", &obj! { "id" => b }), "state"), "draft");

    // B55: Start on a waiting run looks again at once, not 30 s later.
    let asked = fake.world.lock().unwrap().asked.len();
    let run = call(&app, "playlist.start", &obj! { "id" => a });
    assert_eq!(s(&run, "state"), "running");
    wait_for("a new search", 5, || fake.world.lock().unwrap().asked.len() > asked);
    wait_state(&app, a, &["waiting"], 120);

    // B54: a run waiting to look again can be removed, and B gets its turn.
    call(&app, "playlist.delete", &obj! { "id" => a });
    assert_eq!(call_err(&app, "playlist.get", &obj! { "id" => a }).0, "not_found");
    wait_for("the worker to end", 10, || app.playlist.is_active().is_none());
    let run = call(&app, "playlist.start", &obj! { "id" => b });
    assert_eq!(s(&run, "state"), "running");
    call(&app, "playlist.cancel", &obj! { "id" => b });
    wait_state(&app, b, &["cancelled"], 30);
    app.shutdown();
    let _ = std::fs::remove_dir_all(&e.root);
}

#[test]
fn a_run_gets_over_slskd_restarting_and_a_track_removed_while_checked() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let e = env("hiccups");
    let (app, fake) = open(&e);
    setup(&app, &e);
    let id = call(&app, "playlist.fetch", &obj! { "source" => "PLfixtureLateNight" }).get("id").and_then(Value::as_i64).unwrap();
    wait_state(&app, id, &["draft"], 20);
    only(&app, id, &["Rain"]);
    call(&app, "settings.update", &obj! { "values" => obj! { "playlistRules" => obj! { "searchSeconds" => 15, "retryNotFound" => false } } });

    // B14: the first search finds slskd restarting; once it answers, the
    // run no longer says it could not reach it.
    fake.world.lock().unwrap().refuse = 1;
    call(&app, "playlist.start", &obj! { "id" => id });
    wait_for("the error", 10, || !s(&call(&app, "playlist.get", &obj! { "id" => id }), "error").is_empty());
    // Still working (downloading or checking Rain), and no longer saying it.
    wait_for("the error to go while the run works", 60, || {
        let r = call(&app, "playlist.get", &obj! { "id" => id });
        s(&r, "state") == "running" && r.get("error") == Some(&Value::Null) && !matches!(s(item(&r, "Rain"), "status"), "pending" | "searching")
    });
    let run = wait_state(&app, id, &["finished", "failed", "cancelled"], 120);
    assert_eq!(s(&run, "state"), "finished");
    assert_eq!(run.get("error"), Some(&Value::Null), "{:?}", run.get("error"));
    assert_eq!(count(&run, "done"), 1);

    // B13: a track removed from the library while it is being checked
    // ends its check instead of keeping the run going for ever.
    let rain = item(&run, "Rain");
    let (item_id, track_id) = (rain.get("id").and_then(Value::as_i64).unwrap(), rain.get("trackId").and_then(Value::as_i64).unwrap());
    {
        let c = app.db.write();
        c.execute("UPDATE playlist_items SET status = 'verifying', note = NULL, progress_at = ? WHERE id = ?", sonora_core::params![sonora_core::db::now_ms(), item_id]).unwrap();
        c.execute("UPDATE playlist_runs SET state = 'paused', finished_at = NULL WHERE id = ?", sonora_core::params![id]).unwrap();
    }
    call(&app, "tracks.remove", &obj! { "ids" => Value::Array(vec![Value::Int(track_id)]) });
    call(&app, "playlist.resume", &obj! { "id" => id });
    let run = wait_state(&app, id, &["finished"], 30);
    let rain = item(&run, "Rain");
    assert_eq!(s(rain, "status"), "done");
    assert!(s(rain, "note").contains("removed it from the library"), "{:?}", rain.get("note"));
    app.shutdown();
    let _ = std::fs::remove_dir_all(&e.root);
}
