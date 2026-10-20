//! The person's own Soulseek client's download folder: SONORA adds the
//! files that are finished, notes where they came from, and never touches
//! the network. No Soulseek connection is made here or anywhere in SONORA.

use sonora_core::{App, AppConfig};
use sonora_engine::decode::wav::write_wav;
use sonora_engine::synth::{quantize16, render, SynthOptions};
use sonora_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

struct Env {
    root: PathBuf,
    music: PathBuf,
    downloads: PathBuf,
    data: PathBuf,
}

fn env(name: &str) -> Env {
    // Looks happen only when asked, so each step below is the next look.
    std::env::set_var("SONORA_SOURCE_POLL_SECONDS", "0");
    let root = std::env::temp_dir().join(format!("sonora-sources-it-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let music = root.join("Music");
    let downloads = root.join("Soulseek Downloads");
    let data = root.join("data");
    for d in [&music, &downloads, &data] {
        std::fs::create_dir_all(d).unwrap();
    }
    Env { root, music, downloads, data }
}

fn open(e: &Env) -> Arc<App> {
    App::open(AppConfig { data_dir: e.data.clone(), tool_dirs: vec![], app_version: "test".into() }).unwrap()
}

/// A short WAV file, last changed `age` ago (finished downloads are old enough).
fn write(p: &Path, seed: u64, age: Duration) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    let samples = quantize16(&render(&SynthOptions { seconds: 6.0, seed, variant: seed as u32, ..Default::default() }));
    write_wav(p, 44_100, 2, 16, false, &samples).unwrap();
    age_file(p, age);
}

fn age_file(p: &Path, age: Duration) {
    std::fs::File::options().write(true).open(p).unwrap().set_modified(SystemTime::now() - age).unwrap();
}

const OLD: Duration = Duration::from_secs(120);

fn call(app: &Arc<App>, method: &str, params: &str) -> Value {
    let out = app.call(method, params).unwrap_or_else(|e| panic!("{method}: {e}"));
    sonora_json::parse(&out).unwrap()
}

fn call_err(app: &Arc<App>, method: &str, params: &str) -> String {
    app.call(method, params).expect_err(method).code
}

fn wait_idle(app: &Arc<App>) {
    let deadline = Instant::now() + Duration::from_secs(120);
    std::thread::sleep(Duration::from_millis(30));
    loop {
        let s = call(app, "scan.status", "");
        if s.get("state").and_then(Value::as_str) == Some("idle") && s.get("indexing").is_none_or(Value::is_null) {
            return;
        }
        assert!(Instant::now() < deadline, "scan did not finish: {}", s.to_string_compact());
        std::thread::sleep(Duration::from_millis(40));
    }
}

fn names(app: &Arc<App>) -> Vec<String> {
    let v = call(app, "library.query", r#"{"filter": null, "sort": {"key": "path"}, "limit": 500}"#);
    let mut n: Vec<String> = v.get("rows").and_then(Value::as_array).cloned().unwrap_or_default().iter().map(|r| s(r, "fileName").to_string()).collect();
    n.sort();
    n
}

fn id_of(app: &Arc<App>, name: &str) -> i64 {
    let v = call(app, "library.query", r#"{"filter": null, "sort": {"key": "path"}, "limit": 500}"#);
    v.get("rows").and_then(Value::as_array).unwrap().iter().find(|r| s(r, "fileName") == name).and_then(|r| r.get("id").and_then(Value::as_i64)).unwrap()
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

fn found(v: &Value, k: &str) -> i64 {
    v.get("found").and_then(|f| f.get(k)).and_then(Value::as_i64).unwrap_or(-1)
}

fn configure(app: &Arc<App>, folder: &Path, review: bool) -> Value {
    call(app, "sources.configure", &sonora_json::obj! { "folder" => folder.to_string_lossy().to_string(), "review" => review }.to_string_compact())
}

#[test]
fn finished_downloads_join_the_library_with_their_source() {
    let e = env("finished");
    let song = e.downloads.join("Some User/An Album/01 Song.wav");
    let partial = e.downloads.join("incomplete/02 Partial.wav");
    let fresh = e.downloads.join("03 Fresh.wav");
    let app = open(&e);

    // Nothing happens before the lawful-use notice is accepted.
    assert_eq!(call_err(&app, "sources.configure", &sonora_json::obj! { "folder" => e.downloads.to_string_lossy().to_string() }.to_string_compact()), "not_accepted");
    let st = call(&app, "sources.status", "");
    assert_eq!(st.get("accepted").and_then(Value::as_bool), Some(false));
    call(&app, "sources.accept", "");
    // SONORA's own data folder is never watched.
    assert_eq!(call_err(&app, "sources.configure", &sonora_json::obj! { "folder" => e.data.to_string_lossy().to_string() }.to_string_compact()), "invalid_params");
    let st = configure(&app, &e.downloads, false);
    assert_eq!(st.get("watching").and_then(Value::as_bool), Some(true));

    // Downloads arrive after the folder was chosen.
    write(&song, 1, OLD);
    write(&partial, 2, OLD);
    write(&fresh, 3, Duration::ZERO);

    // The first look only notes the files; the partial download is never one.
    let r = call(&app, "sources.check", "");
    assert_eq!((found(&r, "imported"), found(&r, "settling")), (0, 2), "{}", r.to_string_compact());
    // The second look adds the finished file; the fresh one is still settling.
    let r = call(&app, "sources.check", "");
    assert_eq!((found(&r, "imported"), found(&r, "settling")), (1, 1), "{}", r.to_string_compact());
    wait_idle(&app);
    assert_eq!(names(&app), vec!["01 Song.wav".to_string()]);
    let detail = call(&app, "tracks.get", &format!(r#"{{"id": {}, "visuals": false}}"#, id_of(&app, "01 Song.wav")));
    let origin = detail.get("origin").cloned().unwrap_or(Value::Null);
    assert_eq!((s(&origin, "source"), s(&origin, "originalName")), ("soulseek", "01 Song.wav"), "{}", detail.to_string_compact());
    assert_eq!(detail.get("track").and_then(|t| t.get("status")).and_then(Value::as_str), Some("done"), "analysed like any other track");

    // Once it has settled, the other file follows; nothing is added twice.
    age_file(&fresh, OLD);
    call(&app, "sources.check", "");
    let r = call(&app, "sources.check", "");
    assert_eq!(found(&r, "imported"), 1, "{}", r.to_string_compact());
    wait_idle(&app);
    assert_eq!(names(&app), vec!["01 Song.wav".to_string(), "03 Fresh.wav".to_string()]);
    let st = call(&app, "sources.status", "");
    assert_eq!(st.get("counts").and_then(|c| c.get("imported")).and_then(Value::as_i64), Some(2));

    // A track the person removes stays out, as it does for rescans.
    call(&app, "tracks.remove", &format!(r#"{{"ids": [{}]}}"#, id_of(&app, "01 Song.wav")));
    for _ in 0..2 {
        let r = call(&app, "sources.check", "");
        assert_eq!(found(&r, "imported"), 0);
    }
    wait_idle(&app);
    assert_eq!(names(&app), vec!["03 Fresh.wav".to_string()]);
    // The files in the client's folder are never touched.
    for f in [&song, &partial, &fresh] {
        assert!(f.exists(), "{}", f.display());
    }
    app.shutdown();
    let _ = std::fs::remove_dir_all(&e.root);
}

#[test]
fn review_holds_downloads_until_the_person_chooses() {
    let e = env("review");
    let app = open(&e);
    call(&app, "sources.accept", "");
    configure(&app, &e.downloads, true);
    write(&e.downloads.join("keep.wav"), 11, OLD);
    write(&e.downloads.join("skip.wav"), 12, OLD);
    call(&app, "sources.check", "");
    let r = call(&app, "sources.check", "");
    assert_eq!((found(&r, "held"), found(&r, "imported")), (2, 0), "{}", r.to_string_compact());
    let held = call(&app, "sources.review", "");
    let held = held.as_array().unwrap();
    assert_eq!(held.len(), 2);
    let path = |name: &str| held.iter().find(|h| s(h, "name") == name).map(|h| s(h, "path").to_string()).unwrap();

    let r = call(&app, "sources.decide", &sonora_json::obj! { "paths" => vec![path("keep.wav")], "add" => true }.to_string_compact());
    assert_eq!(r.get("added").and_then(Value::as_i64), Some(1));
    let r = call(&app, "sources.decide", &sonora_json::obj! { "paths" => vec![path("skip.wav")], "add" => false }.to_string_compact());
    assert_eq!(r.get("declined").and_then(Value::as_i64), Some(1));
    wait_idle(&app);
    assert_eq!(names(&app), vec!["keep.wav".to_string()]);
    // A file kept out stays out at later looks.
    call(&app, "sources.check", "");
    call(&app, "sources.check", "");
    wait_idle(&app);
    assert_eq!(names(&app), vec!["keep.wav".to_string()]);
    assert_eq!(call(&app, "sources.review", "").as_array().map(Vec::len), Some(0));
    let counts = call(&app, "sources.status", "").get("counts").cloned().unwrap();
    assert_eq!((counts.get("imported").and_then(Value::as_i64), counts.get("declined").and_then(Value::as_i64)), (Some(1), Some(1)));
    app.shutdown();
    let _ = std::fs::remove_dir_all(&e.root);
}

#[test]
fn files_already_there_stay_out_until_added() {
    let e = env("existing");
    // The folder already holds music when it is chosen (another client's
    // older downloads, say), and a partial download.
    write(&e.downloads.join("Older/old.wav"), 31, OLD);
    write(&e.downloads.join("incomplete/part.wav"), 32, OLD);
    let app = open(&e);
    call(&app, "sources.accept", "");
    let st = configure(&app, &e.downloads, false);
    assert_eq!(st.get("existing").and_then(Value::as_i64), Some(1), "{}", st.to_string_compact());
    for _ in 0..2 {
        let r = call(&app, "sources.check", "");
        assert_eq!((found(&r, "imported"), found(&r, "settling")), (0, 0), "{}", r.to_string_compact());
    }
    wait_idle(&app);
    assert!(names(&app).is_empty(), "nothing added on its own");

    // A new download still joins; the older file waits until the person adds it.
    write(&e.downloads.join("new.wav"), 33, OLD);
    call(&app, "sources.check", "");
    call(&app, "sources.check", "");
    wait_idle(&app);
    assert_eq!(names(&app), vec!["new.wav".to_string()]);
    let r = call(&app, "sources.addExisting", "");
    assert_eq!(r.get("added").and_then(Value::as_i64), Some(1));
    wait_idle(&app);
    assert_eq!(names(&app), vec!["new.wav".to_string(), "old.wav".to_string()]);
    let detail = call(&app, "tracks.get", &format!(r#"{{"id": {}, "visuals": false}}"#, id_of(&app, "old.wav")));
    assert_eq!(detail.get("origin").map(|o| s(o, "source")), Some("soulseek"), "added from the download folder on request");
    assert_eq!(call(&app, "sources.status", "").get("existing").and_then(Value::as_i64), Some(0));
    app.shutdown();
    let _ = std::fs::remove_dir_all(&e.root);
}

#[test]
fn files_already_in_the_library_only_get_their_source() {
    let e = env("inside");
    // The download folder is inside a library folder.
    let inside = e.music.join("From Soulseek");
    write(&inside.join("before.wav"), 21, OLD);
    let app = open(&e);
    let import = sonora_json::obj! { "paths" => vec![e.music.to_string_lossy().to_string()] }.to_string_compact();
    call(&app, "library.import", &import);
    wait_idle(&app);
    call(&app, "sources.accept", "");
    let st = configure(&app, &inside, false);
    assert_eq!(st.get("existing").and_then(Value::as_i64), Some(0), "library tracks are not offered again");

    // A download that arrives later, and that a library rescan adds first.
    write(&inside.join("after.wav"), 22, OLD);
    call(&app, "library.import", &import);
    wait_idle(&app);
    assert_eq!(names(&app), vec!["after.wav".to_string(), "before.wav".to_string()]);
    let r = call(&app, "sources.check", "");
    assert_eq!(found(&r, "imported"), 0);
    wait_idle(&app);
    assert_eq!(names(&app), vec!["after.wav".to_string(), "before.wav".to_string()], "no second copy");
    let origin = |name: &str| {
        let d = call(&app, "tracks.get", &format!(r#"{{"id": {}, "visuals": false}}"#, id_of(&app, name)));
        d.get("origin").map(|o| s(o, "source").to_string()).unwrap_or_default()
    };
    assert_eq!(origin("after.wav"), "soulseek");
    assert_eq!(origin("before.wav"), "", "music that was there before is not marked as a download");
    app.shutdown();
    let _ = std::fs::remove_dir_all(&e.root);
}

#[test]
fn a_folder_that_is_gone_is_reported() {
    let e = env("gone");
    let app = open(&e);
    call(&app, "sources.accept", "");
    configure(&app, &e.downloads, false);
    std::fs::remove_dir_all(&e.downloads).unwrap();
    let r = call(&app, "sources.check", "");
    assert!(s(&r, "error").contains("not available"), "{}", r.to_string_compact());
    assert_eq!(r.get("watching").and_then(Value::as_bool), Some(false));
    // Stopping the watch clears it.
    let st = call(&app, "sources.configure", r#"{"folder": null}"#);
    assert!(st.get("folder").is_some_and(Value::is_null));
    app.shutdown();
    let _ = std::fs::remove_dir_all(&e.root);
}
