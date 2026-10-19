//! Playlists through the core's JSON API: the tree, tracks and their order,
//! smart playlists, sorting the library into playlists, and arranging a
//! playlist for mixing.

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
}

fn env(name: &str) -> Env {
    let root = std::env::temp_dir().join(format!("sonora-core-playlists-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (music, data) = (root.join("Music"), root.join("data"));
    for d in [&music, &data] {
        std::fs::create_dir_all(d).unwrap();
    }
    Env { music, data }
}

fn open(e: &Env) -> Arc<App> {
    App::open(AppConfig { data_dir: e.data.clone(), tool_dirs: vec![], app_version: "test".into() }).unwrap()
}

fn call(app: &Arc<App>, method: &str, params: Value) -> Value {
    let out = app.call(method, &params.to_string_compact()).unwrap_or_else(|e| panic!("{method}: {e}"));
    sonora_json::parse(&out).unwrap()
}

fn call_err(app: &Arc<App>, method: &str, params: Value) -> String {
    app.call(method, &params.to_string_compact()).expect_err(method).code
}

fn wait_idle(app: &Arc<App>) {
    let deadline = Instant::now() + Duration::from_secs(180);
    std::thread::sleep(Duration::from_millis(30));
    loop {
        let s = call(app, "scan.status", Value::Null);
        if s.get("state").and_then(Value::as_str) == Some("idle") && s.get("indexing").is_none_or(Value::is_null) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for the scan");
        std::thread::sleep(Duration::from_millis(40));
    }
}

fn write(p: &Path, seconds: f64) {
    let opts = SynthOptions { seconds, bpm: 120.0, ..Default::default() };
    let audio = quantize16(&render(&opts));
    let mut w = PcmWriter::create(p, PcmContainer::Wav, opts.sample_rate, 2, 16, None, None).unwrap();
    w.write(&audio).unwrap();
    w.finish().unwrap();
}

fn int(v: &Value, k: &str) -> i64 {
    v.get(k).and_then(Value::as_i64).unwrap_or_else(|| panic!("no {k} in {}", v.to_string_compact()))
}

fn text<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

/// Track ids by title, after importing `titles` as short WAV files.
fn library(app: &Arc<App>, e: &Env, titles: &[&str]) -> Vec<i64> {
    for t in titles {
        write(&e.music.join(format!("{t}.wav")), 3.0);
    }
    call(app, "library.import", obj! { "paths" => vec![e.music.to_string_lossy().to_string()] });
    wait_idle(app);
    let rows = call(app, "library.query", obj! { "sort" => obj! { "key" => "title" }, "limit" => 100 });
    let rows = rows.get("rows").and_then(Value::as_array).cloned().unwrap();
    titles.iter().map(|t| int(rows.iter().find(|r| text(r, "displayTitle") == *t).unwrap(), "id")).collect()
}

/// Sets tempo, key (0–23) and genre as a DJ would correct them.
fn tag(app: &Arc<App>, id: i64, bpm: f64, key: i64, genre: &str) {
    call(app, "tracks.editDj", obj! { "ids" => vec![id], "bpm" => bpm, "key" => key, "genre" => genre });
}

fn nodes(app: &Arc<App>) -> Vec<Value> {
    call(app, "playlists.list", Value::Null).get("nodes").and_then(Value::as_array).cloned().unwrap()
}

fn node<'a>(list: &'a [Value], name: &str) -> &'a Value {
    list.iter().find(|n| text(n, "name") == name).unwrap_or_else(|| panic!("no playlist {name}"))
}

fn order(app: &Arc<App>, id: i64) -> Vec<i64> {
    let v = call(app, "playlists.tracks", obj! { "id" => id });
    v.get("rows").and_then(Value::as_array).unwrap().iter().map(|r| int(r, "id")).collect()
}

fn create(app: &Arc<App>, kind: &str, name: &str, parent: Option<i64>) -> i64 {
    int(&call(app, "playlists.create", obj! { "kind" => kind, "name" => name, "parentId" => parent }), "id")
}

#[test]
fn the_tree_tracks_and_their_order() {
    let e = env("tree");
    let app = open(&e);
    let t = library(&app, &e, &["One", "Two", "Three", "Four"]);

    let sets = create(&app, "folder", "Sets", None);
    let warm = create(&app, "playlist", "  Warm-up\u{7} ", Some(sets));
    let peak = create(&app, "playlist", "Peak", None);
    let list = nodes(&app);
    assert_eq!(text(node(&list, "Warm-up"), "kind"), "playlist", "names are cleaned");
    assert_eq!(node(&list, "Warm-up").get("parentId").and_then(Value::as_i64), Some(sets));
    assert_eq!((int(node(&list, "Sets"), "position"), int(node(&list, "Peak"), "position")), (0, 1));
    assert_eq!(call_err(&app, "playlists.create", obj! { "kind" => "playlist", "name" => "In a playlist", "parentId" => peak }), "invalid_params");
    assert_eq!(call_err(&app, "playlists.create", obj! { "kind" => "playlist", "name" => "   " }), "invalid_params");

    // Tracks: each once, in the order given, at the end or at an index.
    let r = call(&app, "playlists.addTracks", obj! { "id" => warm, "trackIds" => vec![t[0], t[1], t[2]] });
    assert_eq!((int(&r, "added"), int(&r, "already")), (3, 0));
    let r = call(&app, "playlists.addTracks", obj! { "id" => warm, "trackIds" => vec![t[1], t[3], 999_999] });
    assert_eq!((int(&r, "added"), int(&r, "already")), (1, 1), "unknown tracks are ignored");
    assert_eq!(order(&app, warm), vec![t[0], t[1], t[2], t[3]]);
    call(&app, "playlists.addTracks", obj! { "id" => peak, "trackIds" => vec![t[2]] });
    call(&app, "playlists.addTracks", obj! { "id" => peak, "trackIds" => vec![t[0]], "index" => 0 });
    assert_eq!(order(&app, peak), vec![t[0], t[2]]);

    // Order: move a few, or give the whole order.
    call(&app, "playlists.moveTracks", obj! { "id" => warm, "trackIds" => vec![t[3]], "index" => 0 });
    assert_eq!(order(&app, warm), vec![t[3], t[0], t[1], t[2]]);
    call(&app, "playlists.moveTracks", obj! { "id" => warm, "trackIds" => vec![t[3], t[0]], "index" => 2 });
    assert_eq!(order(&app, warm), vec![t[1], t[2], t[3], t[0]]);
    call(&app, "playlists.reorder", obj! { "id" => warm, "trackIds" => vec![t[0], t[1], t[2], t[3]] });
    assert_eq!(order(&app, warm), vec![t[0], t[1], t[2], t[3]]);
    assert_eq!(call_err(&app, "playlists.reorder", obj! { "id" => warm, "trackIds" => vec![t[0], t[1], t[2]] }), "invalid_params");
    assert_eq!(call_err(&app, "playlists.reorder", obj! { "id" => warm, "trackIds" => vec![t[0], t[0], t[1], t[2]] }), "invalid_params");
    let r = call(&app, "playlists.removeTracks", obj! { "id" => warm, "trackIds" => vec![t[0]] });
    assert_eq!(int(&r, "removed"), 1);
    assert_eq!(order(&app, warm), vec![t[1], t[2], t[3]]);
    let holding = call(&app, "playlists.containing", obj! { "trackId" => t[2] });
    assert_eq!(holding.get("ids").and_then(Value::as_array).map(Vec::len), Some(2));

    // Counts: folders add up what is under them.
    let list = nodes(&app);
    assert_eq!(int(node(&list, "Warm-up"), "count"), 3);
    assert_eq!(int(node(&list, "Sets"), "count"), 3);
    assert_eq!(int(node(&list, "Sets"), "children"), 1);
    assert!(int(node(&list, "Warm-up"), "duration") >= 8);

    // Moving: into folders only, never into itself.
    call(&app, "playlists.move", obj! { "id" => peak, "parentId" => sets, "index" => 0 });
    let list = nodes(&app);
    assert_eq!(node(&list, "Peak").get("parentId").and_then(Value::as_i64), Some(sets));
    assert_eq!((int(node(&list, "Peak"), "position"), int(node(&list, "Warm-up"), "position")), (0, 1));
    assert_eq!(call_err(&app, "playlists.move", obj! { "id" => sets, "parentId" => peak, "index" => 0 }), "invalid_params");
    assert_eq!(call_err(&app, "playlists.move", obj! { "id" => sets, "parentId" => sets, "index" => 0 }), "invalid_params");
    let inner = create(&app, "folder", "Inner", Some(sets));
    assert_eq!(call_err(&app, "playlists.move", obj! { "id" => sets, "parentId" => inner, "index" => 0 }), "invalid_params");

    // Renaming and duplicating.
    call(&app, "playlists.update", obj! { "id" => warm, "name" => "Opening" });
    let copy = int(&call(&app, "playlists.duplicate", obj! { "id" => warm }), "id");
    let list = nodes(&app);
    assert_eq!(text(node(&list, "Opening copy"), "kind"), "playlist");
    assert_eq!(int(node(&list, "Opening copy"), "position"), int(node(&list, "Opening"), "position") + 1);
    assert_eq!(order(&app, copy), order(&app, warm));

    // Removing a track from the library removes it from playlists; the file stays.
    call(&app, "tracks.remove", obj! { "ids" => vec![t[2]] });
    assert_eq!(order(&app, warm), vec![t[1], t[3]]);
    assert!(e.music.join("Three.wav").exists());

    // Everything survives a restart.
    drop(app);
    let app = open(&e);
    assert_eq!(order(&app, warm), vec![t[1], t[3]]);
    assert_eq!(nodes(&app).len(), 5);

    // Deleting a folder deletes what is in it, never tracks or files.
    let r = call(&app, "playlists.delete", obj! { "id" => sets });
    assert_eq!(int(&r, "deleted"), 5);
    assert!(nodes(&app).is_empty());
    let all = call(&app, "library.query", obj! { "limit" => 100 });
    assert_eq!(int(&all, "total"), 3);
    assert!(e.music.join("One.wav").exists());
}

#[test]
fn smart_playlists_sorting_into_playlists_and_arranging() {
    let e = env("smart");
    let app = open(&e);
    let t = library(&app, &e, &["Alpha", "Bravo", "Charlie", "Delta", "Echo", "Foxtrot"]);
    // Keys: 21 = A minor (8A), 16 = E minor (9A), 0 = C major (8B), 3 = E♭ major (5B), 23 = B minor (10A).
    tag(&app, t[0], 124.0, 21, "House");
    tag(&app, t[1], 128.0, 23, "Techno");
    tag(&app, t[2], 126.0, 16, "house");
    tag(&app, t[3], 125.0, 0, "House");
    tag(&app, t[4], 100.0, 3, "Downtempo");
    tag(&app, t[5], 122.0, 21, "Techno");

    // A smart playlist lists what matches, in its sort order.
    let rules = obj! { "filter" => obj! { "bpmMin" => 120, "bpmMax" => 125 }, "sort" => obj! { "key" => "bpm", "desc" => false } };
    let smart = int(&call(&app, "playlists.create", obj! { "kind" => "smart", "name" => "120–125", "rules" => rules }), "id");
    assert_eq!(order(&app, smart), vec![t[5], t[0], t[3]]);
    assert_eq!(int(node(&nodes(&app), "120–125"), "count"), 3);
    assert_eq!(call_err(&app, "playlists.addTracks", obj! { "id" => smart, "trackIds" => vec![t[1]] }), "invalid_params");
    assert_eq!(call_err(&app, "playlists.arrange", obj! { "id" => smart }), "invalid_params");
    let bad = obj! { "filter" => obj! { "playlistId" => smart } };
    assert_eq!(call_err(&app, "playlists.create", obj! { "kind" => "smart", "name" => "Loop", "rules" => bad }), "invalid_params");
    let bad = obj! { "sort" => obj! { "key" => "title; DROP TABLE tracks" } };
    assert_eq!(call_err(&app, "playlists.update", obj! { "id" => smart, "rules" => bad }), "invalid_params");
    // It follows the library: a new tempo moves a track in or out.
    tag(&app, t[1], 121.0, 23, "Techno");
    assert_eq!(order(&app, smart), vec![t[1], t[5], t[0], t[3]]);
    // Converted, it keeps what it listed.
    let r = call(&app, "playlists.freeze", obj! { "id" => smart });
    assert_eq!(int(&r, "tracks"), 4);
    tag(&app, t[1], 128.0, 23, "Techno");
    assert_eq!(order(&app, smart), vec![t[1], t[5], t[0], t[3]]);
    assert_eq!(text(node(&nodes(&app), "120–125"), "kind"), "playlist");

    // Sorting the library into playlists: by genre (case-insensitive), key and tempo.
    let r = call(&app, "playlists.organize", obj! { "by" => "genre" });
    assert_eq!((int(&r, "playlists"), int(&r, "tracks"), int(&r, "without")), (3, 6, 0));
    let list = nodes(&app);
    let folder = int(&r, "folderId");
    assert_eq!(text(node(&list, "By genre"), "kind"), "folder");
    let kids: Vec<&Value> = list.iter().filter(|n| n.get("parentId").and_then(Value::as_i64) == Some(folder)).collect();
    assert_eq!(kids.iter().map(|n| text(n, "name")).collect::<Vec<_>>(), vec!["Downtempo", "House", "Techno"]);
    assert!(kids.iter().all(|n| text(n, "kind") == "smart"));
    assert_eq!(int(node(&list, "House"), "count"), 3);

    let r = call(&app, "playlists.organize", obj! { "by" => "key", "keyNotation" => "camelot" });
    assert_eq!(int(&r, "playlists"), 5);
    let list = nodes(&app);
    assert_eq!(int(node(&list, "8A (A minor)"), "count"), 2);
    assert!(list.iter().any(|n| text(n, "name") == "10A (B minor)"));

    let r = call(&app, "playlists.organize", obj! { "by" => "tempo", "smart" => false, "trackIds" => vec![t[0], t[2], t[3], t[4]] });
    assert_eq!((int(&r, "playlists"), int(&r, "tracks")), (3, 4));
    let list = nodes(&app);
    let band = node(&list, "120–124 BPM");
    assert_eq!(text(band, "kind"), "playlist");
    assert_eq!(order(&app, int(band, "id")), vec![t[0]]);
    assert_eq!(order(&app, int(node(&list, "125–129 BPM"), "id")), vec![t[3], t[2]], "slowest first");
    assert_eq!(order(&app, int(node(&list, "100–104 BPM"), "id")), vec![t[4]]);
    let again = call(&app, "playlists.organize", obj! { "by" => "tempo" });
    assert_eq!(text(node(&nodes(&app), "By tempo 2"), "kind"), "folder", "{}", again.to_string_compact());

    // Arranging for mixing: better transitions, nothing saved until asked.
    let set = create(&app, "playlist", "Set", None);
    let shuffled = vec![t[1], t[4], t[0], t[2], t[3], t[5]];
    call(&app, "playlists.addTracks", obj! { "id" => set, "trackIds" => shuffled.clone() });
    let r = call(&app, "playlists.arrange", obj! { "id" => set });
    let arranged: Vec<i64> = r.get("order").and_then(Value::as_array).unwrap().iter().filter_map(Value::as_i64).collect();
    let (before, after) = (r.get("before").unwrap(), r.get("after").unwrap());
    assert_eq!(arranged.len(), 6);
    assert!(int(after, "harmonic") > int(before, "harmonic"), "{}", r.to_string_compact());
    assert!(after.get("tempoChange").and_then(Value::as_f64) < before.get("tempoChange").and_then(Value::as_f64));
    assert_eq!(order(&app, set), shuffled, "arranging only proposes");
    let pinned = call(&app, "playlists.arrange", obj! { "id" => set, "start" => t[4], "tempo" => "rising" });
    assert_eq!(pinned.get("order").and_then(Value::as_array).and_then(|a| a.first()).and_then(Value::as_i64), Some(t[4]));
    call(&app, "playlists.reorder", obj! { "id" => set, "trackIds" => arranged.clone() });
    assert_eq!(order(&app, set), arranged);
}
