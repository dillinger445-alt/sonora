//! Sending playlists to a drive through the core's JSON API: folders of
//! numbered files, playlist files, and bringing the drive up to date
//! without ever touching files SONORA did not write.

use sonora_core::{App, AppConfig};
use sonora_engine::export::writer::{PcmContainer, PcmWriter};
use sonora_engine::synth::{quantize16, render, SynthOptions};
use sonora_json::{obj, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

struct Env {
    music: PathBuf,
    data: PathBuf,
    drive: PathBuf,
}

fn env(name: &str) -> Env {
    let root = std::env::temp_dir().join(format!("sonora-core-usb-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (music, data, drive) = (root.join("Music"), root.join("data"), root.join("USB"));
    for d in [&music, &data, &drive] {
        std::fs::create_dir_all(d).unwrap();
    }
    Env { music, data, drive }
}

fn open(e: &Env) -> Arc<App> {
    App::open(AppConfig { data_dir: e.data.clone(), tool_dirs: vec![], app_version: "test".into() }).unwrap()
}

fn call(app: &Arc<App>, method: &str, params: Value) -> Value {
    let out = app.call(method, &params.to_string_compact()).unwrap_or_else(|e| panic!("{method}: {e}"));
    sonora_json::parse(&out).unwrap()
}

fn wait<F: Fn() -> bool>(what: &str, done: F) {
    let deadline = Instant::now() + Duration::from_secs(180);
    std::thread::sleep(Duration::from_millis(30));
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(40));
    }
}

fn write(p: &Path, seconds: f64, bpm: f64) {
    let opts = SynthOptions { seconds, bpm, ..Default::default() };
    let audio = quantize16(&render(&opts));
    let mut w = PcmWriter::create(p, PcmContainer::Wav, opts.sample_rate, 2, 16, None, None).unwrap();
    w.write(&audio).unwrap();
    w.finish().unwrap();
}

fn int(v: &Value, k: &str) -> i64 {
    v.get(k).and_then(Value::as_i64).unwrap_or_else(|| panic!("no {k} in {}", v.to_string_compact()))
}

fn library(app: &Arc<App>, e: &Env, titles: &[&str]) -> Vec<i64> {
    for (i, t) in titles.iter().enumerate() {
        write(&e.music.join(format!("{t}.wav")), 2.0, 110.0 + i as f64 * 7.0);
    }
    call(app, "library.import", obj! { "paths" => vec![e.music.to_string_lossy().to_string()] });
    wait("the scan", || {
        let s = call(app, "scan.status", Value::Null);
        s.get("state").and_then(Value::as_str) == Some("idle") && s.get("indexing").is_none_or(Value::is_null)
    });
    let rows = call(app, "library.query", obj! { "sort" => obj! { "key" => "title" }, "limit" => 100 });
    let rows = rows.get("rows").and_then(Value::as_array).cloned().unwrap();
    titles.iter().map(|t| int(rows.iter().find(|r| r.get("displayTitle").and_then(Value::as_str) == Some(*t)).unwrap(), "id")).collect()
}

fn create(app: &Arc<App>, kind: &str, name: &str, parent: Option<i64>, tracks: &[i64]) -> i64 {
    int(&call(app, "playlists.create", obj! { "kind" => kind, "name" => name, "parentId" => parent, "trackIds" => tracks.to_vec() }), "id")
}

fn options(e: &Env, ids: &[i64], remove_stale: bool) -> Value {
    obj! {
        "root" => e.drive.to_string_lossy().to_string(),
        "playlistIds" => ids.to_vec(),
        "format" => "wav",
        "quality" => "universal",
        "keyNotation" => "camelot",
        "rekordboxXml" => true,
        "m3u" => true,
        "serato" => true,
        "removeStale" => remove_stale,
    }
}

fn send(app: &Arc<App>, params: Value) -> Value {
    call(app, "usb.start", params);
    wait("the sync", || call(app, "usb.status", Value::Null).get("running").and_then(Value::as_bool) == Some(false));
    let report = call(app, "usb.status", Value::Null).get("last").cloned().unwrap();
    assert!(report.get("error").is_none_or(Value::is_null), "{}", report.to_string_compact());
    report
}

/// The audio files on the drive below SONORA/, relative, sorted.
fn files(e: &Env) -> Vec<String> {
    fn walk(dir: &Path, base: &Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let p = entry.path();
            if p.is_dir() {
                walk(&p, base, out);
            } else if p.extension().is_some_and(|x| x == "wav") {
                out.push(p.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let mut out = Vec::new();
    let top = e.drive.join("SONORA");
    if top.is_dir() {
        walk(&top, &e.drive, &mut out);
    }
    out.sort();
    out
}

fn counts(plan: &Value) -> (i64, i64, i64, i64, i64) {
    let f = plan.get("files").unwrap();
    (int(f, "new"), int(f, "update"), int(f, "rename"), int(f, "keep"), int(f, "stale"))
}

#[test]
fn playlists_become_folders_and_stay_in_step() {
    let e = env("sync");
    let app = open(&e);
    let t = library(&app, &e, &["One", "Two", "Three", "Four"]);
    let sets = create(&app, "folder", "Sets", None, &[]);
    let warm = create(&app, "playlist", "Warm-up", Some(sets), &[t[0], t[1], t[2]]);
    let peak = create(&app, "playlist", "Peak: time?", None, &[t[2], t[3]]);
    // Something of the user's on the drive, outside SONORA's folder.
    std::fs::write(e.drive.join("notes.txt"), "mine").unwrap();

    // What it would do, then do it.
    let plan = call(&app, "usb.plan", options(&e, &[sets, peak], false));
    assert!(plan.get("problem").is_none_or(Value::is_null), "{}", plan.to_string_compact());
    assert_eq!(counts(&plan), (5, 0, 0, 0, 0));
    assert!(int(plan.get("bytes").unwrap(), "write") > 0);
    let dirs: Vec<&str> = plan.get("playlists").and_then(Value::as_array).unwrap().iter().filter_map(|p| p.get("dir").and_then(Value::as_str)).collect();
    assert_eq!(dirs, vec!["SONORA/01 Sets/01 Warm-up", "SONORA/02 Peak time"]);
    assert_eq!(plan.get("seratoPossible").and_then(Value::as_bool), Some(false), "a folder, not the top of a drive");
    let r = send(&app, options(&e, &[sets, peak], false));
    assert_eq!((int(&r, "written"), int(&r, "playlists"), int(&r, "m3u"), int(&r, "crates")), (5, 2, 2, 0));
    assert_eq!(
        files(&e),
        vec![
            "SONORA/01 Sets/01 Warm-up/001 - One.wav",
            "SONORA/01 Sets/01 Warm-up/002 - Two.wav",
            "SONORA/01 Sets/01 Warm-up/003 - Three.wav",
            "SONORA/02 Peak time/001 - Three.wav",
            "SONORA/02 Peak time/002 - Four.wav",
        ]
    );
    let m3u = std::fs::read_to_string(e.drive.join("SONORA/01 Sets/01 Warm-up/Warm-up.m3u8")).unwrap();
    assert!(m3u.starts_with("#EXTM3U\n#PLAYLIST:Warm-up\n#EXTINF:2,One\n001 - One.wav\n"), "{m3u}");
    let xml = std::fs::read_to_string(e.drive.join("SONORA/rekordbox.xml")).unwrap();
    assert!(xml.contains(r#"<COLLECTION Entries="5">"#), "{xml}");
    assert!(xml.contains(r#"<NODE Name="01 Warm-up" Type="1" KeyType="0" Entries="3">"#), "{xml}");
    assert!(xml.contains(r#"<NODE Type="0" Name="01 Sets" Count="1">"#), "{xml}");
    assert!(e.drive.join("SONORA/.sonora-sync.json").is_file());

    // Nothing changed: nothing to do.
    assert_eq!(counts(&call(&app, "usb.plan", options(&e, &[sets, peak], false))), (0, 0, 0, 5, 0));

    // A new order renames, without writing anything again.
    let before = std::fs::read(e.drive.join("SONORA/01 Sets/01 Warm-up/003 - Three.wav")).unwrap();
    call(&app, "playlists.reorder", obj! { "id" => warm, "trackIds" => vec![t[2], t[0], t[1]] });
    assert_eq!(counts(&call(&app, "usb.plan", options(&e, &[sets, peak], false))), (0, 0, 3, 2, 0));
    let r = send(&app, options(&e, &[sets, peak], false));
    assert_eq!((int(&r, "written"), int(&r, "renamed")), (0, 3));
    assert_eq!(std::fs::read(e.drive.join("SONORA/01 Sets/01 Warm-up/001 - Three.wav")).unwrap(), before);
    assert!(e.drive.join("SONORA/01 Sets/01 Warm-up/002 - One.wav").is_file());

    // A changed tempo writes that file again (in both playlists holding it).
    call(&app, "tracks.editDj", obj! { "ids" => vec![t[2]], "bpm" => 128.0 });
    assert_eq!(counts(&call(&app, "usb.plan", options(&e, &[sets, peak], false))), (0, 2, 0, 3, 0));
    let r = send(&app, options(&e, &[sets, peak], false));
    assert_eq!(int(&r, "written"), 2);
    assert_ne!(std::fs::read(e.drive.join("SONORA/01 Sets/01 Warm-up/001 - Three.wav")).unwrap(), before, "new tags");
    assert_eq!(files(&e).len(), 5);

    // A file of the user's where a new one would go stays untouched.
    std::fs::write(e.drive.join("SONORA/02 Peak time/003 - One.wav"), "not SONORA's").unwrap();
    call(&app, "playlists.addTracks", obj! { "id" => peak, "trackIds" => vec![t[0]] });
    let r = send(&app, options(&e, &[sets, peak], false));
    assert_eq!(int(&r, "written"), 1);
    assert_eq!(std::fs::read_to_string(e.drive.join("SONORA/02 Peak time/003 - One.wav")).unwrap(), "not SONORA's");
    assert!(e.drive.join("SONORA/02 Peak time/003 - One (2).wav").is_file());
    assert_eq!(counts(&call(&app, "usb.plan", options(&e, &[sets, peak], false))), (0, 0, 0, 6, 0), "its copy keeps the (2)");

    // Tracks that left a playlist stay on the drive until removal is asked for.
    call(&app, "playlists.removeTracks", obj! { "id" => warm, "trackIds" => vec![t[1]] });
    let plan = call(&app, "usb.plan", options(&e, &[sets, peak], false));
    assert_eq!(counts(&plan).4, 1);
    send(&app, options(&e, &[sets, peak], false));
    assert!(e.drive.join("SONORA/01 Sets/01 Warm-up/003 - Two.wav").is_file());
    let r = send(&app, options(&e, &[sets, peak], true));
    assert_eq!(int(&r, "removed"), 1);
    assert!(!e.drive.join("SONORA/01 Sets/01 Warm-up/003 - Two.wav").exists());

    // A deleted playlist's files count as left; its folder goes once empty
    // of SONORA's files. Sending one playlist leaves the others alone.
    call(&app, "playlists.delete", obj! { "id" => sets });
    let plan = call(&app, "usb.plan", options(&e, &[peak], true));
    assert_eq!(counts(&plan).4, 2, "{}", plan.to_string_compact());
    send(&app, options(&e, &[peak], true));
    assert!(!e.drive.join("SONORA/01 Sets").exists());
    assert_eq!(
        files(&e),
        vec!["SONORA/01 Peak time/001 - Three.wav", "SONORA/01 Peak time/002 - Four.wav", "SONORA/01 Peak time/003 - One.wav", "SONORA/02 Peak time/003 - One.wav"],
        "renumbered to 01, and the user's file stays where it was"
    );
    assert!(e.drive.join("SONORA/01 Peak time/Peak time.m3u8").is_file());
    assert!(!e.drive.join("SONORA/02 Peak time/Peak time.m3u8").exists(), "SONORA's outdated playlist file goes");
    assert_eq!(std::fs::read_to_string(e.drive.join("notes.txt")).unwrap(), "mine");
}

#[test]
fn drives_are_listed_and_space_is_known() {
    // Whatever is attached here; the call must work on every system.
    let e = env("drives");
    let app = open(&e);
    let v = call(&app, "usb.drives", Value::Null);
    assert!(v.get("drives").and_then(Value::as_array).is_some());
    if cfg!(any(unix, windows)) {
        assert!(sonora_core::usb::space(&e.drive).is_some_and(|(total, free)| total > 0 && free <= total));
    }
}

#[test]
fn a_stopped_send_and_a_changed_playlist_file_leave_the_drive_in_order() {
    let e = env("stopped");
    let app = open(&e);
    let t = library(&app, &e, &["One", "Two", "Three", "Four", "Five", "Six"]);
    let warm = create(&app, "playlist", "Warm-up", None, &t);
    // B11: when the second file starts, the drive's record already holds the
    // first, so a send cut short there still knows what it wrote.
    let seen = Arc::new(std::sync::Mutex::new(None::<String>));
    let (record, look) = (e.drive.join("SONORA/.sonora-sync.json"), seen.clone());
    let sub = app.events.subscribe(Box::new(move |name, payload| {
        if name == "usb.progress" && payload.contains("\"done\":1,") && look.lock().unwrap().is_none() {
            *look.lock().unwrap() = std::fs::read_to_string(&record).ok();
        }
    }));
    send(&app, options(&e, &[warm], false));
    app.events.unsubscribe(sub);
    let at_second = seen.lock().unwrap().clone().unwrap_or_default();
    assert!(at_second.contains("001 - One.wav"), "{at_second}");
    let m3u_path = e.drive.join("SONORA/01 Warm-up/Warm-up.m3u8");
    let listed = |p: &Path| std::fs::read_to_string(p).unwrap().lines().filter(|l| l.ends_with(".wav")).count();
    assert_eq!(listed(&m3u_path), 6);

    // B10: the person edits SONORA's playlist file. It stays theirs; SONORA
    // writes its own beside it, under the same other name every time.
    let mut edited = std::fs::read_to_string(&m3u_path).unwrap();
    edited.push_str("# my notes\n");
    std::fs::write(&m3u_path, &edited).unwrap();
    let r = send(&app, options(&e, &[warm], false));
    let warnings = |r: &Value| r.get("warnings").and_then(Value::as_array).map(|w| w.len()).unwrap_or(0);
    assert_eq!(warnings(&r), 1, "{}", r.to_string_compact());
    let second = e.drive.join("SONORA/01 Warm-up/Warm-up (2).m3u8");
    assert!(second.is_file());
    let r = send(&app, options(&e, &[warm], false));
    assert_eq!(warnings(&r), 0, "said once: {}", r.to_string_compact());
    assert!(second.is_file(), "the same name again");
    assert!(!e.drive.join("SONORA/01 Warm-up/Warm-up (3).m3u8").exists());
    assert_eq!(std::fs::read_to_string(&m3u_path).unwrap(), edited, "the person's file is untouched");

    // B05: every file needs updating (another key notation); the send stops
    // after the first. The playlists still list all six files, which are all
    // still there (B11: and the drive's record knows every file written).
    let weak = Arc::downgrade(&app);
    let sub = app.events.subscribe(Box::new(move |name, payload| {
        if name == "usb.progress" && payload.contains("\"done\":1,") {
            if let Some(app) = weak.upgrade() {
                app.syncer.cancel();
            }
        }
    }));
    let mut opts = options(&e, &[warm], false);
    opts.set("keyNotation", "musical");
    call(&app, "usb.start", opts);
    wait("the sync", || call(&app, "usb.status", Value::Null).get("running").and_then(Value::as_bool) == Some(false));
    app.events.unsubscribe(sub);
    let r = call(&app, "usb.status", Value::Null).get("last").cloned().unwrap();
    assert_eq!(r.get("cancelled").and_then(Value::as_bool), Some(true), "{}", r.to_string_compact());
    assert!(int(&r, "written") < 6, "{}", r.to_string_compact());
    assert_eq!(files(&e).len(), 6);
    assert_eq!(listed(&second), 6, "every file on the drive is still in the playlist");
    let xml = std::fs::read_to_string(e.drive.join("SONORA/rekordbox.xml")).unwrap();
    assert!(xml.contains(r#"<COLLECTION Entries="6">"#), "{xml}");
    // What was not reached is still to do; nothing written is someone else's.
    let mut opts = options(&e, &[warm], false);
    opts.set("keyNotation", "musical");
    let plan = call(&app, "usb.plan", opts);
    let (new, update, _, keep, _) = counts(&plan);
    assert_eq!((new, update + keep), (0, 6), "{}", plan.to_string_compact());
    assert!(plan.get("warnings").and_then(Value::as_array).is_none_or(|w| w.is_empty()), "{}", plan.to_string_compact());
}

#[test]
fn quitting_during_a_send_lets_it_put_the_drive_in_order() {
    // B12: shutdown waits for the send to stop and tidy up.
    let e = env("quit");
    let app = open(&e);
    let t = library(&app, &e, &["One", "Two", "Three", "Four", "Five", "Six"]);
    let warm = create(&app, "playlist", "Warm-up", None, &t);
    send(&app, options(&e, &[warm], false));
    // Every file needs updating: the old ones step aside while new ones are written.
    let mut opts = options(&e, &[warm], false);
    opts.set("keyNotation", "musical");
    call(&app, "usb.start", opts);
    app.shutdown();
    assert!(!app.syncer.is_running(), "shutdown returned while the send was still going");
    let mut hidden = Vec::new();
    for entry in std::fs::read_dir(e.drive.join("SONORA/01 Warm-up")).unwrap().flatten() {
        let n = entry.file_name().to_string_lossy().to_string();
        if n.starts_with('.') {
            hidden.push(n);
        }
    }
    assert!(hidden.is_empty(), "{hidden:?}");
    assert_eq!(files(&e).len(), 6, "every track still has its file");
}

#[test]
fn an_export_plan_says_when_a_send_is_running() {
    // B48: the export dialog plans with this, and must not offer Export
    // while a send to a drive is under way.
    let e = env("busy");
    let app = open(&e);
    let t = library(&app, &e, &["One", "Two", "Three"]);
    let warm = create(&app, "playlist", "Warm-up", None, &t);
    let seen = Arc::new(std::sync::Mutex::new(None::<bool>));
    let (weak, look) = (Arc::downgrade(&app), seen.clone());
    let ids = t.clone();
    let sub = app.events.subscribe(Box::new(move |name, _| {
        if name == "usb.progress" && look.lock().unwrap().is_none() {
            if let Some(app) = weak.upgrade() {
                let plan = app.call("djexport.plan", &obj! { "ids" => ids.clone(), "format" => "aiff" }.to_string_compact()).unwrap();
                *look.lock().unwrap() = sonora_json::parse(&plan).unwrap().get("running").and_then(Value::as_bool);
            }
        }
    }));
    send(&app, options(&e, &[warm], false));
    app.events.unsubscribe(sub);
    assert_eq!(*seen.lock().unwrap(), Some(true));
    let plan = call(&app, "djexport.plan", obj! { "ids" => t.clone(), "format" => "aiff" });
    assert_eq!(plan.get("running").and_then(Value::as_bool), Some(false));
}
