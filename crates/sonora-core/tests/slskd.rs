//! Searching and downloading through slskd, against a fake slskd: no
//! network and no Soulseek. The fake answers the way slskd's API does and
//! records what SONORA asked of it.

use sonora_core::net::{Http, NetError, Request, Response};
use sonora_core::{App, AppConfig};
use sonora_engine::decode::wav::write_wav;
use sonora_engine::synth::{quantize16, render, SynthOptions};
use sonora_json::{obj, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

const KEY: &str = "test-key-0123456789abcdef";
const SEARCH_ID: &str = "0b9a3c34-5f39-4d1e-9a51-6d1f2e3a4b5c";
const TRANSFER_ID: &str = "7c1d2e3f-4a5b-4c6d-8e9f-0a1b2c3d4e5f";

/// What the fake slskd does and what it was asked.
#[derive(Default)]
struct State {
    requests: Vec<(String, String, Option<String>)>,
    unreachable: bool,
    /// The size of the downloaded file once it "finished".
    finished: Option<i64>,
    /// Looks at the search so far: slskd stores the responses only once the
    /// search has ended, after a moment of "Completed" without them.
    search_looks: usize,
}

#[derive(Default)]
struct FakeSlskd {
    state: Mutex<State>,
}

fn json(status: u16, v: Value) -> Response {
    Response { status, body: v.to_string_compact() }
}

fn result_file(name: &str, size: i64) -> Value {
    obj! { "filename" => name, "size" => size, "bitDepth" => 16, "sampleRate" => 44100, "length" => 6, "extension" => "" }
}

impl Http for FakeSlskd {
    fn send(&self, req: &Request) -> Result<Response, NetError> {
        let mut st = self.state.lock().unwrap();
        if st.unreachable {
            return Err(NetError::Unreachable("curl: (7) Failed to connect".into()));
        }
        let path = req.url.strip_prefix("http://localhost:5030/api/v0/").expect("the address set in the test").to_string();
        st.requests.push((req.method.to_string(), path.clone(), req.body.clone()));
        let key = req.headers.iter().find(|(k, _)| k == "X-API-Key").map(|(_, v)| v.as_str());
        if key != Some(KEY) {
            return Ok(Response { status: 401, body: String::new() });
        }
        Ok(match (req.method, path.as_str()) {
            ("GET", "application") => json(
                200,
                obj! {
                    "version" => obj! { "current" => "0.26.1" },
                    "server" => obj! { "state" => "Connected, LoggedIn", "isConnected" => true, "isLoggedIn" => true },
                    "user" => obj! { "username" => "me" },
                    "shares" => obj! { "directories" => 3, "files" => 120 },
                },
            ),
            ("POST", "searches") => json(200, obj! { "id" => SEARCH_ID, "searchText" => "artist song", "state" => "InProgress" }),
            ("GET", p) if p == format!("searches/{SEARCH_ID}?includeResponses=true") => {
                st.search_looks += 1;
                let mut v = obj! { "id" => SEARCH_ID, "searchText" => "artist song", "responseCount" => 2 };
                match st.search_looks {
                    1 => v.set("state", "InProgress"),
                    2 => v.set("state", "Completed, TimedOut"),
                    _ => {
                        v.set("state", "Completed, TimedOut");
                        v.set("endedAt", "2026-10-20T10:00:15.5Z");
                        v.set(
                            "responses",
                            Value::Array(vec![
                                obj! {
                                    "username" => "sharer", "hasFreeUploadSlot" => true, "queueLength" => 0, "uploadSpeed" => 1_500_000,
                                    "files" => Value::Array(vec![result_file("@@abc\\Music\\Artist - Album\\01 Song.flac", st.finished.unwrap_or(1_234_567)), result_file("@@abc\\Music\\Artist - Album\\cover.jpg", 50_000)]),
                                },
                                obj! {
                                    "username" => "other", "hasFreeUploadSlot" => false, "queueLength" => 4, "uploadSpeed" => 100_000,
                                    "files" => Value::Array(vec![result_file("mp3s\\Artist\\01 Song.mp3", 5_000_000)]),
                                },
                            ]),
                        );
                    }
                }
                json(200, v)
            }
            ("PUT", p) if p == format!("searches/{SEARCH_ID}") => Response { status: 200, body: String::new() },
            ("POST", "transfers/downloads/sharer") => json(201, obj! { "enqueued" => Value::Array(vec![obj! { "id" => TRANSFER_ID }]), "failed" => Value::Array(vec![]) }),
            ("POST", p) if p.starts_with("transfers/downloads/") => Response { status: 500, body: "\"User nobody appears to be offline\"".into() },
            ("GET", "transfers/downloads") => {
                let (state, size) = match st.finished {
                    Some(size) => ("Completed, Succeeded", size),
                    None => ("Queued, Remotely", 1_234_567),
                };
                json(
                    200,
                    Value::Array(vec![obj! {
                        "username" => "sharer",
                        "directories" => Value::Array(vec![obj! {
                            "directory" => "@@abc\\Music\\Artist - Album",
                            "fileCount" => 1,
                            "files" => Value::Array(vec![obj! {
                                "id" => TRANSFER_ID,
                                "username" => "sharer",
                                "direction" => "Download",
                                "filename" => "@@abc\\Music\\Artist - Album\\01 Song.flac",
                                "size" => size,
                                "state" => state,
                                "requestedAt" => "2026-10-20T10:00:00.1234567",
                                "endedAt" => if st.finished.is_some() { Value::from("2026-10-20T10:02:00Z") } else { Value::Null },
                                "bytesTransferred" => if st.finished.is_some() { size } else { 0 },
                                "averageSpeed" => 512_000.5,
                                "placeInQueue" => if st.finished.is_some() { Value::Null } else { Value::from(3) },
                            }]),
                        }]),
                    }]),
                )
            }
            ("DELETE", p) if p == format!("transfers/downloads/sharer/{TRANSFER_ID}") => Response { status: 204, body: String::new() },
            ("DELETE", "transfers/downloads/all/completed") => Response { status: 204, body: String::new() },
            _ => Response { status: 404, body: String::new() },
        })
    }
}

struct Env {
    root: PathBuf,
    downloads: PathBuf,
    data: PathBuf,
}

fn env(name: &str) -> Env {
    std::env::set_var("SONORA_SOURCE_POLL_SECONDS", "0");
    std::env::set_var("SONORA_SECRET_STORE", "file");
    let root = std::env::temp_dir().join(format!("sonora-slskd-it-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let downloads = root.join("slskd downloads");
    let data = root.join("data");
    std::fs::create_dir_all(&downloads).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    Env { root, downloads, data }
}

fn open(e: &Env) -> (Arc<App>, Arc<FakeSlskd>) {
    let app = App::open(AppConfig { data_dir: e.data.clone(), tool_dirs: vec![], app_version: "test".into() }).unwrap();
    let fake = Arc::new(FakeSlskd::default());
    app.slskd.set_http(fake.clone());
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

fn wait_idle(app: &Arc<App>) {
    let deadline = Instant::now() + Duration::from_secs(120);
    std::thread::sleep(Duration::from_millis(30));
    loop {
        let st = call(app, "scan.status", &obj! {});
        if s(&st, "state") == "idle" && st.get("indexing").is_none_or(Value::is_null) {
            return;
        }
        assert!(Instant::now() < deadline, "scan did not finish");
        std::thread::sleep(Duration::from_millis(40));
    }
}

/// The downloaded file: a short WAV (SONORA reads files by their content, so
/// the .flac name does not matter here).
fn write_download(p: &Path) -> i64 {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    let samples = quantize16(&render(&SynthOptions { seconds: 6.0, seed: 7, variant: 7, ..Default::default() }));
    write_wav(p, 44_100, 2, 16, false, &samples).unwrap();
    std::fs::File::options().write(true).open(p).unwrap().set_modified(SystemTime::now() - Duration::from_secs(120)).unwrap();
    std::fs::metadata(p).unwrap().len() as i64
}

#[test]
fn search_download_and_find_the_track() {
    let e = env("flow");
    let (app, fake) = open(&e);

    // Nothing is asked of slskd before the notice is accepted and a key saved.
    assert_eq!(call_err(&app, "slskd.search", &obj! { "text" => "artist song" }).0, "not_accepted");
    let st = call(&app, "slskd.status", &obj! {});
    assert_eq!((s(&st, "url"), st.get("keySaved").and_then(Value::as_bool), s(&st, "keyStore")), ("http://localhost:5030", Some(false), "file"));
    assert_eq!(call_err(&app, "slskd.test", &obj! {}).0, "slskd_not_set_up");
    call(&app, "sources.accept", &obj! {});
    assert_eq!(call_err(&app, "slskd.configure", &obj! { "apiKey" => "short" }).0, "invalid_params");
    assert_eq!(call_err(&app, "slskd.configure", &obj! { "url" => "localhost:5030" }).0, "invalid_params");
    let st = call(&app, "slskd.configure", &obj! { "url" => "http://localhost:5030/", "apiKey" => KEY });
    assert_eq!(st.get("keySaved").and_then(Value::as_bool), Some(true));
    assert_eq!(st.get("local").and_then(Value::as_bool), Some(true));
    assert!(fake.state.lock().unwrap().requests.is_empty(), "configuring asks nothing of slskd");

    let t = call(&app, "slskd.test", &obj! {});
    assert_eq!((s(&t, "username"), t.get("loggedIn").and_then(Value::as_bool), s(&t, "version")), ("me", Some(true), "0.26.1"));
    assert_eq!(t.get("sharedFiles").and_then(Value::as_i64), Some(120));

    // The key stays out of the library database.
    let db = std::fs::read(e.data.join("sonora.db")).unwrap();
    assert!(!db.windows(KEY.len()).any(|w| w == KEY.as_bytes()), "the API key is in the database");

    // slskd's download folder, chosen while it is empty.
    call(&app, "sources.configure", &obj! { "folder" => e.downloads.to_string_lossy().to_string() });

    // Search: the words go to slskd; the results come back grouped, lossless only.
    assert_eq!(call_err(&app, "slskd.search", &obj! { "text" => "  " }).0, "invalid_params");
    let r = call(&app, "slskd.search", &obj! { "text" => "artist song" });
    assert_eq!(s(&r, "id"), SEARCH_ID);
    let body = fake.state.lock().unwrap().requests.last().unwrap().2.clone().unwrap();
    assert_eq!(sonora_json::parse(&body).unwrap().get("searchText").and_then(Value::as_str), Some("artist song"));
    assert_eq!(call_err(&app, "slskd.results", &obj! { "id" => "../application" }).0, "invalid_params");
    // Until slskd has stored the responses, the search is still on, even
    // when slskd already says "Completed".
    for _ in 0..2 {
        let res = call(&app, "slskd.results", &obj! { "id" => SEARCH_ID });
        assert_eq!((s(&res, "state"), res.get("responses").and_then(Value::as_i64)), ("searching", Some(2)), "{}", res.to_string_compact());
        assert_eq!(res.get("folders").and_then(Value::as_array).map(Vec::len), Some(0));
    }
    let res = call(&app, "slskd.results", &obj! { "id" => SEARCH_ID });
    assert_eq!(s(&res, "state"), "done");
    let folders = res.get("folders").and_then(Value::as_array).unwrap();
    assert_eq!(folders.len(), 1, "{}", res.to_string_compact());
    assert_eq!((s(&folders[0], "username"), s(&folders[0], "name")), ("sharer", "Artist - Album"));
    assert_eq!(res.get("hiddenFiles").and_then(Value::as_i64), Some(1), "the MP3");
    let file = &folders[0].get("files").and_then(Value::as_array).unwrap()[0];
    assert_eq!((s(file, "ext"), file.get("lossless").and_then(Value::as_bool), file.get("inLibrary").and_then(Value::as_bool)), ("flac", Some(true), Some(false)));
    let all = call(&app, "slskd.results", &obj! { "id" => SEARCH_ID, "lossless" => false });
    assert_eq!(all.get("folders").and_then(Value::as_array).map(Vec::len), Some(2));
    call(&app, "slskd.stop", &obj! { "id" => SEARCH_ID });

    // Download: slskd gets the user, the remote path and the size.
    let d = call(&app, "slskd.download", &obj! { "username" => "sharer", "files" => Value::Array(vec![obj! { "filename" => s(file, "filename"), "size" => 1_234_567 }]) });
    assert_eq!(d.get("queued").and_then(Value::as_i64), Some(1));
    let (method, path, body) = fake.state.lock().unwrap().requests.last().unwrap().clone();
    assert_eq!((method.as_str(), path.as_str()), ("POST", "transfers/downloads/sharer"));
    let sent = sonora_json::parse(&body.unwrap()).unwrap();
    assert_eq!(sent.as_array().unwrap()[0].get("filename").and_then(Value::as_str), Some("@@abc\\Music\\Artist - Album\\01 Song.flac"));
    let (code, message) = call_err(&app, "slskd.download", &obj! { "username" => "nobody", "files" => Value::Array(vec![obj! { "filename" => "x\\y.flac", "size" => 1 }]) });
    assert_eq!(code, "slskd_failed");
    assert!(message.contains("appears to be offline"), "{message}");
    // The log keeps that slskd failed, never what it said: its answers can
    // name other users. Nor the key.
    let log = std::fs::read_to_string(e.data.join("logs").join("sonora.log")).unwrap_or_default();
    assert!(log.contains("slskd request failed"), "{log}");
    assert!(!log.contains("nobody") && !log.contains("offline") && !log.contains(KEY), "{log}");

    let list = call(&app, "slskd.downloads", &obj! {});
    let item = &list.get("downloads").and_then(Value::as_array).unwrap()[0];
    assert_eq!((s(item, "state"), item.get("position").and_then(Value::as_i64), s(item, "folder")), ("queued", Some(3), "Artist - Album"));
    assert_eq!(list.get("active").and_then(Value::as_i64), Some(1));

    // slskd finishes the file in its download folder; the watcher adds it.
    let local = e.downloads.join("Artist - Album").join("01 Song.flac");
    let size = write_download(&local);
    fake.state.lock().unwrap().finished = Some(size);
    call(&app, "sources.check", &obj! {});
    call(&app, "sources.check", &obj! {});
    wait_idle(&app);
    let list = call(&app, "slskd.downloads", &obj! {});
    let item = &list.get("downloads").and_then(Value::as_array).unwrap()[0];
    assert_eq!(s(item, "state"), "done", "{}", list.to_string_compact());
    let local_info = item.get("local").cloned().unwrap_or(Value::Null);
    assert_eq!(s(&local_info, "state"), "imported", "{}", list.to_string_compact());
    let track = local_info.get("track").cloned().unwrap_or(Value::Null);
    assert_eq!(s(&track, "fileName"), "01 Song.flac");
    let detail = call(&app, "tracks.get", &obj! { "id" => track.get("id").and_then(Value::as_i64).unwrap(), "visuals" => false });
    assert_eq!(detail.get("origin").map(|o| s(o, "source")), Some("soulseek"));

    // The same file in later results is marked as already in the library.
    let res = call(&app, "slskd.results", &obj! { "id" => SEARCH_ID });
    let file = &res.get("folders").and_then(Value::as_array).unwrap()[0].get("files").and_then(Value::as_array).unwrap()[0];
    assert_eq!(file.get("inLibrary").and_then(Value::as_bool), Some(true));

    assert_eq!(call_err(&app, "slskd.cancel", &obj! { "username" => "sharer", "id" => "not-an-id" }).0, "invalid_params");
    call(&app, "slskd.cancel", &obj! { "username" => "sharer", "id" => TRANSFER_ID });
    call(&app, "slskd.clearFinished", &obj! {});
    assert!(local.exists(), "slskd's files are never touched");
    app.shutdown();
    let _ = std::fs::remove_dir_all(&e.root);
}

#[test]
fn problems_are_explained() {
    let e = env("problems");
    let (app, fake) = open(&e);
    call(&app, "sources.accept", &obj! {});
    call(&app, "slskd.configure", &obj! { "apiKey" => "wrong-key-0123456789" });
    let (code, message) = call_err(&app, "slskd.test", &obj! {});
    assert_eq!(code, "slskd_unauthorized");
    assert!(!message.contains("wrong-key"), "the key never appears in messages");
    fake.state.lock().unwrap().unreachable = true;
    let (code, message) = call_err(&app, "slskd.test", &obj! {});
    assert_eq!(code, "slskd_unreachable");
    assert!(message.contains("http://localhost:5030"), "{message}");

    // A new key: letters and digits, saved at once, and removable.
    let k = call(&app, "slskd.newKey", &obj! {});
    let key = s(&k, "key").to_string();
    assert_eq!(key.len(), 32);
    assert!(key.chars().all(|c| c.is_ascii_alphanumeric()));
    let secrets = std::fs::read_to_string(e.data.join("secrets.json")).unwrap();
    assert!(secrets.contains(&key));
    let st = call(&app, "slskd.configure", &obj! { "apiKey" => Value::Null });
    assert_eq!(st.get("keySaved").and_then(Value::as_bool), Some(false));
    assert!(!std::fs::read_to_string(e.data.join("secrets.json")).unwrap().contains(&key));
    app.shutdown();
    let _ = std::fs::remove_dir_all(&e.root);
}
