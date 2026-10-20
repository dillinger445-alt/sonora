//! End-to-end tests of the core through its JSON API, on real audio files.

use sonora_core::{App, AppConfig};
use sonora_engine::decode::wav::write_wav;
use sonora_engine::synth::{quantize16, render, SynthOptions};
use sonora_json::Value;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

struct Env {
    root: PathBuf,
    music: PathBuf,
    data: PathBuf,
}

fn env(name: &str) -> Env {
    let root = std::env::temp_dir().join(format!("sonora-core-it-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let music = root.join("Music");
    let data = root.join("data");
    std::fs::create_dir_all(music.join("Sets/Techno")).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    Env { root, music, data }
}

fn track(seed: u64, seconds: f64) -> Vec<f32> {
    quantize16(&render(&SynthOptions { seconds, seed, variant: seed as u32, ..Default::default() }))
}

fn write(p: &Path, seed: u64, seconds: f64) {
    write_wav(p, 44_100, 2, 16, false, &track(seed, seconds)).unwrap();
}

fn open(e: &Env) -> Arc<App> {
    App::open(AppConfig { data_dir: e.data.clone(), tool_dirs: vec![], app_version: "test".into() }).unwrap()
}

fn call(app: &Arc<App>, method: &str, params: &str) -> Value {
    let out = app.call(method, params).unwrap_or_else(|e| panic!("{method}: {e}"));
    sonora_json::parse(&out).unwrap()
}

fn call_err(app: &Arc<App>, method: &str, params: &str) -> String {
    app.call(method, params).expect_err(method).code
}

fn wait_idle(app: &Arc<App>) {
    let deadline = Instant::now() + Duration::from_secs(120);
    // Give a just-started indexer a moment to register.
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

fn rows(app: &Arc<App>, filter: &str) -> Vec<Value> {
    let v = call(app, "library.query", &format!(r#"{{"filter": {filter}, "sort": {{"key": "path"}}, "limit": 500}}"#));
    v.get("rows").and_then(Value::as_array).cloned().unwrap_or_default()
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

fn import(app: &Arc<App>, path: &Path) {
    let p = sonora_json::obj! { "paths" => vec![path.to_string_lossy().to_string()] };
    call(app, "library.import", &p.to_string_compact());
    wait_idle(app);
}

#[test]
fn import_analyze_persist_and_reopen() {
    let e = env("persist");
    write(&e.music.join("01 Opening.wav"), 1, 12.0);
    write(&e.music.join("Sets/Techno/02 Peak Time.wav"), 2, 12.0);
    std::fs::write(e.music.join("Sets/broken.mp3"), vec![0x55u8; 4096]).unwrap();
    std::fs::write(e.music.join("cover.jpg"), b"not audio").unwrap();

    let app = open(&e);
    let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let ev = events.clone();
    app.subscribe(Box::new(move |name, _payload| ev.lock().unwrap().push(name.to_string())));
    import(&app, &e.music);

    let all = rows(&app, "null");
    assert_eq!(all.len(), 3, "audio candidates only (jpg ignored)");
    let done: Vec<&Value> = all.iter().filter(|r| s(r, "status") == "done").collect();
    let failed: Vec<&Value> = all.iter().filter(|r| s(r, "status") == "failed").collect();
    assert_eq!(done.len(), 2);
    assert_eq!(failed.len(), 1);
    assert!(s(failed[0], "errorMessage").len() > 3);
    for r in &done {
        assert!(r.get("overall").and_then(Value::as_f64).unwrap() >= 90.0);
        assert_eq!(s(r, "format"), "WAV");
    }
    let names = events.lock().unwrap().clone();
    for needed in ["scan.started", "scan.item", "scan.progress", "scan.finished", "library.changed"] {
        assert!(names.iter().any(|n| n == needed), "missing event {needed}: {names:?}");
    }

    // Detail includes the analysis documents and visualization data.
    let id = done[0].get("id").and_then(Value::as_i64).unwrap();
    let detail = call(&app, "tracks.get", &format!(r#"{{"id": {id}}}"#));
    let analysis = detail.get("analysis").unwrap();
    assert!(analysis.get("assessment").and_then(|a| a.get("verdict")).and_then(Value::as_str).is_some());
    assert!(analysis.get("visuals").and_then(|v| v.get("spectrogram")).is_some());
    assert_eq!(detail.get("fileExists").and_then(Value::as_bool), Some(true));

    let overview = call(&app, "app.overview", "");
    assert_eq!(overview.get("tracks").and_then(|t| t.get("analyzed")).and_then(Value::as_i64), Some(2));
    assert_eq!(overview.get("folders").and_then(Value::as_i64), Some(1));
    let history = call(&app, "scan.history", "");
    let first = &history.as_array().unwrap()[0];
    assert_eq!(s(first, "state"), "completed");
    assert_eq!(first.get("completed").and_then(Value::as_i64), Some(2));
    assert_eq!(first.get("failed").and_then(Value::as_i64), Some(1));

    app.shutdown();
    drop(app);
    let app2 = open(&e);
    let again = rows(&app2, "null");
    assert_eq!(again.len(), 3);
    assert_eq!(again.iter().filter(|r| s(r, "status") == "done").count(), 2, "results survive a restart");
    app2.shutdown();
    let _ = std::fs::remove_dir_all(&e.root);
}

#[test]
fn rescan_skips_unchanged_reanalyzes_changed_relinks_moved_and_flags_missing() {
    let e = env("changes");
    let a = e.music.join("a.wav");
    let b = e.music.join("b.wav");
    let c = e.music.join("c.wav");
    write(&a, 11, 10.0);
    write(&b, 12, 10.0);
    write(&c, 13, 10.0);
    let app = open(&e);
    import(&app, &e.music);
    let before = rows(&app, "null");
    assert_eq!(before.len(), 3);
    let analyzed_at = |rows: &[Value], name: &str| rows.iter().find(|r| s(r, "fileName") == name).and_then(|r| r.get("analyzedAt").and_then(Value::as_i64));
    let a_time = analyzed_at(&before, "a.wav");

    // Unchanged: nothing is queued.
    call(&app, "folders.rescan", "{}");
    wait_idle(&app);
    let hist = call(&app, "scan.history", r#"{"limit": 1}"#);
    assert_eq!(hist.as_array().unwrap()[0].get("total").and_then(Value::as_i64), Some(0));
    assert_eq!(hist.as_array().unwrap()[0].get("skipped").and_then(Value::as_i64), Some(3));
    // Its summary has the same shape as one of a scan that analysed files.
    let summary = hist.as_array().unwrap()[0].get("summary").cloned().unwrap_or(Value::Null);
    assert_eq!(s(&summary, "message"), "No new or changed files");
    assert_eq!(summary.get("elapsedMs").and_then(Value::as_i64), Some(0));

    // Modified content → re-analysed; renamed → relinked without analysis;
    // deleted → missing but kept.
    std::thread::sleep(Duration::from_millis(20));
    write(&a, 99, 11.0);
    std::fs::rename(&b, e.music.join("b renamed.wav")).unwrap();
    std::fs::remove_file(&c).unwrap();
    call(&app, "folders.rescan", "{}");
    wait_idle(&app);
    let after = rows(&app, "null");
    assert_eq!(after.len(), 3, "rename keeps one entry: {after:?}");
    assert_ne!(analyzed_at(&after, "a.wav"), a_time, "changed file was re-analysed");
    let renamed = after.iter().find(|r| s(r, "fileName") == "b renamed.wav").expect("relinked");
    assert_eq!(s(renamed, "status"), "done");
    let missing = after.iter().find(|r| s(r, "fileName") == "c.wav").unwrap();
    assert_eq!(missing.get("missing").and_then(Value::as_bool), Some(true));
    assert!(missing.get("overall").and_then(Value::as_f64).is_some(), "results kept for missing files");
    let hist = call(&app, "scan.history", r#"{"limit": 1}"#);
    assert_eq!(hist.as_array().unwrap()[0].get("total").and_then(Value::as_i64), Some(1), "only the modified file was analysed");

    // Removing an index entry never touches the file system.
    let id = renamed.get("id").and_then(Value::as_i64).unwrap();
    let r = call(&app, "tracks.remove", &format!(r#"{{"ids": [{id}]}}"#));
    assert_eq!((r.get("removed").and_then(Value::as_i64), r.get("busy").and_then(Value::as_i64)), (Some(1), Some(0)));
    assert!(e.music.join("b renamed.wav").exists());
    assert_eq!(rows(&app, "null").len(), 2);

    // A removed track stays removed when its folder is rescanned...
    call(&app, "folders.rescan", "{}");
    wait_idle(&app);
    assert_eq!(rows(&app, "null").len(), 2, "a rescan does not bring a removed track back");
    let folders = call(&app, "folders.list", "");
    let folder = &folders.as_array().unwrap()[0];
    assert_eq!(folder.get("removed").and_then(Value::as_i64), Some(1));
    // ...until it is brought back, which rescans the folder.
    let back = call(&app, "folders.bringBack", &format!(r#"{{"id": {}}}"#, folder.get("id").and_then(Value::as_i64).unwrap()));
    assert_eq!(back.get("count").and_then(Value::as_i64), Some(1));
    wait_idle(&app);
    let again = rows(&app, "null");
    assert_eq!(again.len(), 3);
    // Or imported again by itself.
    let id = again.iter().find(|r| s(r, "fileName") == "b renamed.wav").and_then(|r| r.get("id").and_then(Value::as_i64)).unwrap();
    call(&app, "tracks.remove", &format!(r#"{{"ids": [{id}]}}"#));
    call(&app, "folders.rescan", "{}");
    wait_idle(&app);
    assert_eq!(rows(&app, "null").len(), 2);
    import(&app, &e.music.join("b renamed.wav"));
    assert_eq!(rows(&app, "null").len(), 3);
    assert_eq!(call(&app, "folders.list", "").as_array().unwrap()[0].get("removed").and_then(Value::as_i64), Some(0));
    let id = rows(&app, "null").iter().find(|r| s(r, "fileName") == "b renamed.wav").and_then(|r| r.get("id").and_then(Value::as_i64)).unwrap();
    call(&app, "tracks.remove", &format!(r#"{{"ids": [{id}]}}"#));
    assert_eq!(rows(&app, "null").len(), 2);

    // Files dropped by name that are not audio stay out, and are counted.
    std::fs::write(e.music.join("cover.jpg"), b"\xFF\xD8\xFF\xE0\0\x10JFIF").unwrap();
    std::fs::write(e.music.join("._a.wav"), b"\0\x05\x16\x07").unwrap();
    let reports: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = reports.clone();
    let sub = app.subscribe(Box::new(move |name, payload| {
        if name == "library.indexed" {
            sink.lock().unwrap().push(payload.to_string());
        }
    }));
    call(
        &app,
        "library.import",
        &sonora_json::obj! { "paths" => vec![e.music.join("cover.jpg").to_string_lossy().to_string(), e.music.join("._a.wav").to_string_lossy().to_string()] }.to_string_compact(),
    );
    wait_idle(&app);
    app.unsubscribe(sub);
    assert_eq!(rows(&app, "null").len(), 2, "no track for a cover image or a macOS metadata file");
    let report = sonora_json::parse(reports.lock().unwrap().last().expect("an index report")).unwrap();
    assert_eq!((int(&report, "notAudio"), int(&report, "added"), int(&report, "queued")), (2, 0, 0));

    // Missing filter.
    assert_eq!(rows(&app, r#"{"statuses": ["missing"]}"#).len(), 1);
    app.shutdown();
    let _ = std::fs::remove_dir_all(&e.root);
}

/// The trash tests point XDG_DATA_HOME (process-wide) at a folder of their
/// own: one at a time.
static TRASH_ENV: Mutex<()> = Mutex::new(());

#[test]
fn trashing_moves_files_and_takes_their_tracks_out() {
    let _env = TRASH_ENV.lock().unwrap_or_else(|p| p.into_inner());
    let e = env("trash");
    // Elsewhere than macOS and Windows, the freedesktop trash: kept in this
    // test's folder. (The system Trash and Recycle Bin are used as they are.)
    #[cfg(not(any(target_os = "macos", windows)))]
    std::env::set_var("XDG_DATA_HOME", e.root.join("xdg"));
    let (a, b, c) = (e.music.join("a.wav"), e.music.join("b.wav"), e.music.join("c.wav"));
    write(&a, 41, 8.0);
    write(&b, 42, 8.0);
    write(&c, 43, 8.0);
    let app = open(&e);
    import(&app, &e.music);
    let all = rows(&app, "null");
    assert_eq!(all.len(), 3);
    let id = |name: &str| all.iter().find(|r| s(r, "fileName") == name).and_then(|r| r.get("id").and_then(Value::as_i64)).unwrap();

    // One file goes to the Trash; one already gone just leaves the library.
    std::fs::remove_file(&c).unwrap();
    let r = call(&app, "tracks.trash", &format!(r#"{{"ids": [{}, {}]}}"#, id("a.wav"), id("c.wav")));
    assert_eq!((int(&r, "trashed"), int(&r, "removed"), int(&r, "busy")), (1, 2, 0), "{}", r.to_string_compact());
    assert_eq!(r.get("failed").and_then(Value::as_array).map(Vec::len), Some(0));
    assert!(!a.exists(), "the file left its folder");
    assert!(b.exists(), "other files stay");
    assert_eq!(rows(&app, "null").len(), 1);
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let kept = e.root.join("xdg/Trash/files/a.wav");
        assert!(kept.exists(), "the file is in the trash");
        // Put back from the Trash, a rescan finds it again: trashing does not
        // keep it out of the library the way removing does.
        std::fs::rename(&kept, &a).unwrap();
        call(&app, "folders.rescan", "{}");
        wait_idle(&app);
        assert_eq!(rows(&app, "null").len(), 2);
    }

    // A folder that is not there (a disconnected drive) is not a deleted
    // file: the track stays, and the answer says why.
    let away = e.root.join("Unplugged");
    std::fs::create_dir_all(&away).unwrap();
    write(&away.join("d.wav"), 44, 8.0);
    import(&app, &away.join("d.wav"));
    let d = rows(&app, "null").iter().find(|r| s(r, "fileName") == "d.wav").and_then(|r| r.get("id").and_then(Value::as_i64)).unwrap();
    std::fs::rename(&away, e.root.join("Unplugged elsewhere")).unwrap();
    let r = call(&app, "tracks.trash", &format!(r#"{{"ids": [{d}]}}"#));
    assert_eq!((int(&r, "trashed"), int(&r, "removed")), (0, 0), "{}", r.to_string_compact());
    let failed = r.get("failed").and_then(Value::as_array).cloned().unwrap_or_default();
    assert_eq!(failed.len(), 1);
    assert!(s(&failed[0], "error").contains("not available"), "{}", r.to_string_compact());
    assert!(rows(&app, "null").iter().any(|r| s(r, "fileName") == "d.wav"), "the track stays");
    std::fs::rename(e.root.join("Unplugged elsewhere"), &away).unwrap();
    app.shutdown();
    let _ = std::fs::remove_dir_all(&e.root);
}

/// A file that cannot go to the Trash keeps its place in a running scan.
#[cfg(not(any(target_os = "macos", windows)))]
#[test]
fn a_file_that_stays_keeps_its_place_in_the_scan() {
    let _env = TRASH_ENV.lock().unwrap_or_else(|p| p.into_inner());
    let e = env("trash-queue");
    // A trash that cannot be made: a path under a file.
    std::fs::write(e.root.join("not a folder"), b"x").unwrap();
    std::env::set_var("XDG_DATA_HOME", e.root.join("not a folder/xdg"));
    for i in 0..5 {
        write(&e.music.join(format!("w{i}.wav")), 500 + i, 12.0);
    }
    let app = open(&e);
    call(&app, "settings.update", r#"{"values": {"workers": 1}}"#);
    call(&app, "library.import", &sonora_json::obj! { "paths" => vec![e.music.to_string_lossy().to_string()] }.to_string_compact());
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let st = call(&app, "scan.status", "");
        if s(&st, "state") == "running" && int(&st, "completed") >= 1 {
            break;
        }
        assert!(Instant::now() < deadline, "no file finished: {}", st.to_string_compact());
        std::thread::sleep(Duration::from_millis(10));
    }
    call(&app, "scan.pause", "");
    while int(&call(&app, "scan.status", ""), "running") > 0 {
        assert!(Instant::now() < deadline, "the running file did not finish");
        std::thread::sleep(Duration::from_millis(10));
    }
    let before = call(&app, "scan.status", "");
    let waiting = rows(&app, r#"{"statuses": ["pending"]}"#);
    let id = waiting[0].get("id").and_then(Value::as_i64).unwrap();
    let r = call(&app, "tracks.trash", &format!(r#"{{"ids": [{id}]}}"#));
    assert_eq!((int(&r, "trashed"), int(&r, "removed")), (0, 0), "{}", r.to_string_compact());
    // Still waiting in the same scan, which ends with every file analysed.
    let after = call(&app, "scan.status", "");
    assert_eq!((int(&after, "total"), int(&after, "pending")), (int(&before, "total"), int(&before, "pending")), "{}", after.to_string_compact());
    call(&app, "scan.resume", "");
    wait_idle(&app);
    assert_eq!(rows(&app, r#"{"statuses": ["done"]}"#).len(), 5);
    app.shutdown();
    let _ = std::fs::remove_dir_all(&e.root);
}

/// Folders inside folders: a file removed from the library stays out while
/// any library folder still covers it.
#[test]
fn nested_folders_keep_removed_tracks_out() {
    let e = env("nested");
    // Native separators: on Windows "Sets/Techno" would make paths that differ
    // from the ones a walk of the outer folder finds.
    let techno = e.music.join("Sets").join("Techno");
    write(&e.music.join("top.wav"), 61, 8.0);
    write(&techno.join("one.wav"), 62, 8.0);
    write(&techno.join("two.wav"), 63, 8.0);
    let app = open(&e);
    // The inner folder first: its tracks belong to it.
    import(&app, &techno);
    import(&app, &e.music);
    assert_eq!(rows(&app, "null").len(), 3);
    let folder_id = |path: &Path| {
        call(&app, "folders.list", "")
            .as_array()
            .unwrap()
            .iter()
            .find(|f| s(f, "path") == path.to_string_lossy())
            .and_then(|f| f.get("id").and_then(Value::as_i64))
            .unwrap()
    };
    let names = || {
        let mut n: Vec<String> = rows(&app, "null").iter().map(|r| s(r, "fileName").to_string()).collect();
        n.sort();
        n
    };
    let one = rows(&app, "null").iter().find(|r| s(r, "fileName") == "one.wav").and_then(|r| r.get("id").and_then(Value::as_i64)).unwrap();
    call(&app, "tracks.remove", &format!(r#"{{"ids": [{one}]}}"#));

    // The inner folder stops being watched; the outer one still covers the
    // removed file, so its rescans leave it out.
    call(&app, "folders.remove", &format!(r#"{{"id": {}}}"#, folder_id(&techno)));
    call(&app, "folders.rescan", "{}");
    wait_idle(&app);
    assert_eq!(names(), vec!["top.wav".to_string(), "two.wav".to_string()]);
    // Bring back on the outer folder returns it.
    let back = call(&app, "folders.bringBack", &format!(r#"{{"id": {}}}"#, folder_id(&e.music)));
    assert_eq!(back.get("count").and_then(Value::as_i64), Some(1));
    wait_idle(&app);
    assert_eq!(names().len(), 3);
    app.shutdown();
    let _ = std::fs::remove_dir_all(&e.root);
}

#[test]
fn cancel_keeps_the_database_consistent_and_pending_can_resume() {
    let e = env("cancel");
    for i in 0..8 {
        write(&e.music.join(format!("t{i}.wav")), 100 + i, 20.0);
    }
    let app = open(&e);
    call(&app, "settings.update", r#"{"values": {"workers": 1}}"#);
    call(&app, "library.import", &sonora_json::obj! { "paths" => vec![e.music.to_string_lossy().to_string()] }.to_string_compact());
    // Wait until analysis has started, then cancel.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let st = call(&app, "scan.status", "");
        if s(&st, "state") == "running" {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    call(&app, "scan.cancel", "");
    wait_idle(&app);
    let all = rows(&app, "null");
    assert!(all.iter().all(|r| s(r, "status") != "analyzing"), "no track left mid-analysis");
    let done = all.iter().filter(|r| s(r, "status") == "done").count();
    assert!(done < 8, "cancel took effect before the end");
    let hist = call(&app, "scan.history", r#"{"limit": 1}"#);
    assert_eq!(s(&hist.as_array().unwrap()[0], "state"), "cancelled");

    // Resume the rest.
    call(&app, "settings.update", r#"{"values": {"workers": 2}}"#);
    let q = call(&app, "tracks.analyzePending", "");
    assert_eq!(q.get("queued").and_then(Value::as_i64), Some((8 - done) as i64));
    wait_idle(&app);
    assert_eq!(rows(&app, r#"{"statuses": ["done"]}"#).len(), 8);
    app.shutdown();
    let _ = std::fs::remove_dir_all(&e.root);
}

#[test]
fn pause_and_resume() {
    let e = env("pause");
    for i in 0..4 {
        write(&e.music.join(format!("p{i}.wav")), 200 + i, 15.0);
    }
    let app = open(&e);
    call(&app, "settings.update", r#"{"values": {"workers": 1}}"#);
    call(&app, "library.import", &sonora_json::obj! { "paths" => vec![e.music.to_string_lossy().to_string()] }.to_string_compact());
    let deadline = Instant::now() + Duration::from_secs(30);
    while s(&call(&app, "scan.status", ""), "state") != "running" {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(call(&app, "scan.pause", "").get("ok").and_then(Value::as_bool), Some(true));
    std::thread::sleep(Duration::from_millis(1500));
    let st = call(&app, "scan.status", "");
    assert_eq!(s(&st, "state"), "paused");
    let completed = st.get("completed").and_then(Value::as_i64).unwrap();
    std::thread::sleep(Duration::from_millis(800));
    assert_eq!(call(&app, "scan.status", "").get("completed").and_then(Value::as_i64).unwrap(), completed, "no progress while paused");
    call(&app, "scan.resume", "");
    wait_idle(&app);
    assert_eq!(rows(&app, r#"{"statuses": ["done"]}"#).len(), 4);
    app.shutdown();
    let _ = std::fs::remove_dir_all(&e.root);
}

#[test]
fn query_search_sort_filter_export_and_duplicates() {
    let e = env("query");
    let x = track(7, 12.0);
    write_wav(&e.music.join("Deep House Groove.wav"), 44_100, 2, 16, false, &x).unwrap();
    // Same audio, different container depth → identical decoded audio.
    write_wav(&e.music.join("Groove (24-bit export).wav"), 44_100, 2, 24, false, &x).unwrap();
    // Byte-identical copy.
    std::fs::copy(e.music.join("Deep House Groove.wav"), e.music.join("Sets/Deep House Groove copy.wav")).unwrap();
    // Clipped version (different audio, same recording): ~5 dB into a hard clip.
    let clipped: Vec<f32> = x.iter().map(|v| (v * 1.8).clamp(-1.0, 1.0)).collect();
    write_wav(&e.music.join("Groove CLIPPED.wav"), 44_100, 2, 16, false, &quantize16(&clipped)).unwrap();
    let app = open(&e);
    import(&app, &e.music);

    assert_eq!(rows(&app, r#"{"search": "groove clipped"}"#).len(), 1);
    assert_eq!(rows(&app, r#"{"search": "x'); DROP TABLE tracks; --"}"#).len(), 0);
    assert_eq!(rows(&app, "null").len(), 4, "injection attempt was inert");
    assert_eq!(rows(&app, r#"{"flags": ["clipping"]}"#).len(), 1);
    assert_eq!(rows(&app, r#"{"maxScore": 80}"#).len(), 1);
    let sorted = call(&app, "library.query", r#"{"sort": {"key": "overall", "desc": false}, "limit": 10}"#);
    let first = &sorted.get("rows").unwrap().as_array().unwrap()[0];
    assert!(s(first, "fileName").contains("CLIPPED"));
    assert_eq!(call_err(&app, "library.query", r#"{"sort": {"key": "overall; DROP TABLE x"}}"#), "invalid_params");

    let csv = call(&app, "export.csv", r#"{"filter": {"search": "groove"}}"#);
    let text = s(&csv, "csv");
    assert_eq!(text.lines().count(), 5, "header + 4 rows");
    assert!(text.lines().next().unwrap().starts_with("path,file_name,title"));
    let out = e.root.join("report.csv");
    call(&app, "export.csv", &sonora_json::obj! { "path" => out.to_string_lossy().to_string() }.to_string_compact());
    assert!(std::fs::read_to_string(&out).unwrap().contains("Groove CLIPPED.wav"));
    assert_eq!(call_err(&app, "export.csv", r#"{"path": "relative.csv"}"#), "invalid_params");

    let dups = call(&app, "duplicates.list", "");
    let groups = dups.get("groups").unwrap().as_array().unwrap();
    let kinds: Vec<&str> = groups.iter().map(|g| s(g, "kind")).collect();
    assert!(kinds.contains(&"exact"), "{kinds:?}");
    assert!(kinds.contains(&"same_audio"), "{kinds:?}");
    let exact = groups.iter().find(|g| s(g, "kind") == "exact").unwrap();
    assert_eq!(exact.get("members").unwrap().as_array().unwrap().len(), 2);
    assert!(exact.get("suggestedKeep").and_then(Value::as_i64).is_some());

    // Fingerprint scan: the clipped version is the same recording, the
    // others are already grouped by hash.
    call(&app, "duplicates.scan", "");
    let deadline = Instant::now() + Duration::from_secs(60);
    while call(&app, "duplicates.list", "").get("scanning").and_then(Value::as_bool) == Some(true) {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(50));
    }
    let dups = call(&app, "duplicates.list", "");
    let groups = dups.get("groups").unwrap().as_array().unwrap();
    let fp = groups.iter().find(|g| s(g, "kind") == "same_recording").expect("fingerprint group");
    assert!(fp.get("members").unwrap().as_array().unwrap().iter().any(|m| s(m, "fileName").contains("CLIPPED")));
    app.shutdown();
    let _ = std::fs::remove_dir_all(&e.root);
}

#[test]
fn api_validation_settings_and_recovery() {
    let e = env("api");
    let app = open(&e);
    assert_eq!(call_err(&app, "nope.method", ""), "unknown_method");
    assert_eq!(call_err(&app, "library.import", r#"{"paths": ["relative/dir"]}"#), "invalid_params");
    assert_eq!(call_err(&app, "library.import", r#"{"paths": []}"#), "invalid_params");
    assert_eq!(call_err(&app, "tracks.get", r#"{"id": 12345}"#), "not_found");
    assert_eq!(call_err(&app, "tracks.get", r#"{"id": "x"}"#), "invalid_params");
    assert_eq!(call_err(&app, "settings.update", r#"{"values": {"workers": 0}}"#), "invalid_params");
    assert_eq!(app.call("app.info", "{not json").unwrap_err().code, "invalid_params");
    let st = call(&app, "settings.update", r#"{"values": {"workers": 3, "defaultScore": "dj"}}"#);
    assert_eq!(st.get("workers").and_then(Value::as_i64), Some(3));
    let info = call(&app, "app.info", "");
    assert!(info.get("ffmpeg").is_some());
    app.shutdown();
    drop(app);

    // Simulate a crash mid-scan: a track left "analyzing", a scan "running".
    {
        let db = sonora_core::db::Db::open(&e.data.join("sonora.db")).unwrap();
        let c = db.write();
        c.execute(
            "INSERT INTO tracks (path, file_name, ext, size, mtime, quick_hash, status, added_at) VALUES ('/x/y.wav', 'y.wav', 'wav', 1, 1, 'h', 'analyzing', 1)",
            sonora_core::params![],
        )
        .unwrap();
        c.execute("INSERT INTO scans (kind, state, started_at) VALUES ('import', 'running', 1)", sonora_core::params![]).unwrap();
    }
    let app = open(&e);
    assert_eq!(call(&app, "settings.get", "").get("workers").and_then(Value::as_i64), Some(3), "settings persisted");
    let all = rows(&app, "null");
    assert_eq!(s(&all[0], "status"), "pending", "interrupted analysis returns to pending");
    let hist = call(&app, "scan.history", "");
    assert_eq!(s(&hist.as_array().unwrap()[0], "state"), "interrupted");
    app.shutdown();
    let _ = std::fs::remove_dir_all(&e.root);
}

#[test]
fn outdated_scores_are_refreshed_from_stored_measurements() {
    let e = env("rescore");
    write(&e.music.join("a.wav"), 7, 8.0);
    let app = open(&e);
    import(&app, &e.music);
    let before = rows(&app, "null");
    assert_eq!(before.len(), 1);
    let id = before[0].get("id").and_then(Value::as_i64).unwrap();
    let overall = before[0].get("overall").and_then(Value::as_f64).unwrap();
    let model = s(&before[0], "modelVersion").to_string();

    // Pretend the track was scored by an older model with different results.
    app.db.write().execute("UPDATE tracks SET overall = 1, dj = 1, listening = 1, model_version = 'sonora-dsp-0.9' WHERE id = ?", sonora_core::params![id]).unwrap();
    let n = sonora_core::library::rescore_outdated(&app.db, &sonora_engine::assess::ScoringConfig::default()).unwrap();
    assert_eq!(n, 1);
    let after = rows(&app, "null");
    assert_eq!(after[0].get("overall").and_then(Value::as_f64), Some(overall));
    assert_eq!(s(&after[0], "modelVersion"), model);
    // Nothing left to do.
    assert_eq!(sonora_core::library::rescore_outdated(&app.db, &sonora_engine::assess::ScoringConfig::default()).unwrap(), 0);
}

#[test]
fn interrupted_scans_resume_once_on_the_next_launch() {
    let e = env("resume");
    write(&e.music.join("a.wav"), 11, 6.0);
    write(&e.music.join("b.wav"), 12, 6.0);
    let app = open(&e);
    import(&app, &e.music);
    let ids: Vec<i64> = rows(&app, "null").iter().map(|r| r.get("id").and_then(Value::as_i64).unwrap()).collect();
    assert_eq!(ids.len(), 2);

    // Simulate a crash in the middle of a scan: one track still queued.
    {
        let c = app.db.write();
        c.execute("UPDATE tracks SET status = 'pending' WHERE id = ?", sonora_core::params![ids[1]]).unwrap();
        c.execute("INSERT INTO scans (kind, state, started_at, total) VALUES ('import', 'running', 1, 2)", sonora_core::params![]).unwrap();
        let scan = c.last_insert_rowid();
        c.execute("INSERT INTO scan_items (scan_id, track_id, seq, outcome) VALUES (?, ?, 1, 'done'), (?, ?, 2, 'queued')", sonora_core::params![scan, ids[0], scan, ids[1]]).unwrap();
    }
    app.shutdown();
    drop(app);

    let app = open(&e);
    wait_idle(&app);
    let r = rows(&app, "null");
    assert!(r.iter().all(|t| s(t, "status") == "done"), "the queued track was analyzed after the restart");
    let history = call(&app, "scan.history", "");
    let kinds: Vec<String> = history.as_array().unwrap().iter().map(|h| s(h, "kind").to_string()).collect();
    assert!(kinds.contains(&"resume".to_string()), "{kinds:?}");
    app.shutdown();
    drop(app);

    // Resumed once only: reopening again starts nothing.
    let app = open(&e);
    wait_idle(&app);
    let status = call(&app, "scan.status", "");
    assert_eq!(status.get("state").and_then(Value::as_str), Some("idle"));
    let again = call(&app, "scan.history", "");
    assert_eq!(again.as_array().unwrap().len(), history.as_array().unwrap().len());
}

#[test]
fn quitting_mid_scan_resumes_on_the_next_launch() {
    let e = env("quit");
    for i in 0..4 {
        write(&e.music.join(format!("t{i}.wav")), 20 + i, 20.0);
    }
    let app = open(&e);
    let p = sonora_json::obj! { "paths" => vec![e.music.to_string_lossy().to_string()] };
    call(&app, "library.import", &p.to_string_compact());
    // Wait for the analysis to start, then pause so work is left in the queue.
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let st = call(&app, "scan.status", "");
        if st.get("state").and_then(Value::as_str) == Some("running") {
            break;
        }
        assert!(Instant::now() < deadline, "scan never started");
        std::thread::sleep(Duration::from_millis(5));
    }
    call(&app, "scan.pause", "");
    app.shutdown();
    let history = call(&app, "scan.history", "");
    assert_eq!(s(&history.as_array().unwrap()[0], "state"), "interrupted", "quitting is not a cancel");
    drop(app);

    let app = open(&e);
    wait_idle(&app);
    let r = rows(&app, "null");
    assert_eq!(r.len(), 4);
    assert!(r.iter().all(|t| s(t, "status") == "done"), "{:?}", r.iter().map(|t| s(t, "status").to_string()).collect::<Vec<_>>());
}

fn int(v: &Value, k: &str) -> i64 {
    v.get(k).and_then(Value::as_i64).unwrap_or_else(|| panic!("{k} missing in {}", v.to_string_compact()))
}

#[test]
fn requeued_and_removed_tracks_keep_the_scan_counts_right() {
    let e = env("requeue");
    for i in 0..6 {
        write(&e.music.join(format!("q{i}.wav")), 300 + i, 12.0);
    }
    let app = open(&e);
    call(&app, "settings.update", r#"{"values": {"workers": 1}}"#);
    call(&app, "library.import", &sonora_json::obj! { "paths" => vec![e.music.to_string_lossy().to_string()] }.to_string_compact());
    // Let a file finish, then pause between files.
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let st = call(&app, "scan.status", "");
        if s(&st, "state") == "running" && int(&st, "completed") >= 1 {
            break;
        }
        assert!(Instant::now() < deadline, "no file finished: {}", st.to_string_compact());
        std::thread::sleep(Duration::from_millis(10));
    }
    call(&app, "scan.pause", "");
    while int(&call(&app, "scan.status", ""), "running") > 0 {
        assert!(Instant::now() < deadline, "the running file did not finish");
        std::thread::sleep(Duration::from_millis(10));
    }
    let st = call(&app, "scan.status", "");
    assert_eq!(int(&st, "total"), 6);
    let completed = int(&st, "completed");
    assert_eq!(int(&st, "done"), completed);
    let finished = rows(&app, r#"{"statuses": ["done"]}"#);
    let waiting = rows(&app, r#"{"statuses": ["pending"]}"#);
    assert_eq!(finished.len() as i64, completed);
    assert_eq!(waiting.len() as i64, 6 - completed);
    assert!(waiting.len() >= 3, "{waiting:?}");
    let id = |r: &Value| int(r, "id");

    // A track this session already finished goes back into the queue.
    let q = call(&app, "tracks.rescan", &format!(r#"{{"ids": [{}]}}"#, id(&finished[0])));
    assert_eq!((int(&q, "queued"), int(&q, "alreadyQueued"), int(&q, "missing")), (1, 0, 0));
    let st = call(&app, "scan.status", "");
    assert_eq!((int(&st, "total"), int(&st, "completed"), int(&st, "pending")), (6, completed - 1, 7 - completed));

    // One that is still waiting is reported as already queued.
    let q = call(&app, "tracks.rescan", &format!(r#"{{"ids": [{}]}}"#, id(&waiting[0])));
    assert_eq!((int(&q, "queued"), int(&q, "alreadyQueued")), (0, 1));

    // Removed tracks leave the session's total.
    let r = call(&app, "tracks.remove", &format!(r#"{{"ids": [{}, {}]}}"#, id(&waiting[1]), id(&waiting[2])));
    assert_eq!(int(&r, "removed"), 2);
    let st = call(&app, "scan.status", "");
    assert_eq!((int(&st, "total"), int(&st, "pending")), (4, 5 - completed));

    // Resumed, the session ends with every remaining item analysed once more.
    call(&app, "scan.resume", "");
    wait_idle(&app);
    let hist = call(&app, "scan.history", r#"{"limit": 1}"#);
    let h = &hist.as_array().unwrap()[0];
    assert_eq!(s(h, "state"), "completed");
    assert_eq!((int(h, "total"), int(h, "completed"), int(h, "failed")), (4, 4, 0));
    assert_eq!(rows(&app, r#"{"statuses": ["done"]}"#).len(), 4);

    // Queueing tracks that are no longer in the library does nothing, and
    // leaves no half-started session behind.
    let q = call(&app, "tracks.retryFailed", &format!(r#"{{"ids": [{}, {}]}}"#, id(&waiting[1]), id(&waiting[2])));
    assert_eq!(int(&q, "queued"), 0);
    let st = call(&app, "scan.status", "");
    assert_eq!(s(&st, "state"), "idle");
    assert!(st.get("id").is_none_or(Value::is_null), "{}", st.to_string_compact());
    assert_eq!(call(&app, "scan.history", r#"{"limit": 10}"#).as_array().unwrap().len(), 1, "no empty session recorded");
    // A mix of present and removed tracks queues the present ones; a
    // removed track is not a missing file.
    let q = call(&app, "tracks.rescan", &format!(r#"{{"ids": [{}, {}]}}"#, id(&finished[0]), id(&waiting[1])));
    assert_eq!((int(&q, "queued"), int(&q, "missing")), (1, 0));
    wait_idle(&app);
    assert_eq!(s(&call(&app, "scan.history", r#"{"limit": 1}"#).as_array().unwrap()[0], "state"), "completed");
    app.shutdown();
    let _ = std::fs::remove_dir_all(&e.root);
}
