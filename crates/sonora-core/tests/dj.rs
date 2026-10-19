//! DJ features through the core's JSON API: detected tempo and key, tags,
//! edits, folder genres, filters, the BPM range, and DJ-ready exports.

use sonora_core::{App, AppConfig};
use sonora_engine::analysis::key::{format_key, parse_key, Notation};
use sonora_engine::export::id3::{build, TagSet};
use sonora_engine::export::writer::{PcmContainer, PcmWriter};
use sonora_engine::synth::{quantize16, render, SynthOptions};
use sonora_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

struct Env {
    music: PathBuf,
    data: PathBuf,
    out: PathBuf,
}

fn env(name: &str) -> Env {
    let root = std::env::temp_dir().join(format!("sonora-core-dj-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (music, data, out) = (root.join("Music"), root.join("data"), root.join("Export"));
    for d in [&music, &data, &out] {
        std::fs::create_dir_all(d).unwrap();
    }
    Env { music, data, out }
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

fn wait<F: Fn() -> bool>(what: &str, done: F) {
    let deadline = Instant::now() + Duration::from_secs(180);
    std::thread::sleep(Duration::from_millis(30));
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(40));
    }
}

fn wait_idle(app: &Arc<App>) {
    wait("the scan", || {
        let s = call(app, "scan.status", "");
        s.get("state").and_then(Value::as_str) == Some("idle") && s.get("indexing").is_none_or(Value::is_null)
    });
}

fn import(app: &Arc<App>, path: &Path) {
    call(app, "library.import", &sonora_json::obj! { "paths" => vec![path.to_string_lossy().to_string()] }.to_string_compact());
    wait_idle(app);
}

fn rows(app: &Arc<App>, filter: &str, sort: &str) -> Vec<Value> {
    let v = call(app, "library.query", &format!(r#"{{"filter": {filter}, "sort": {{"key": "{sort}"}}, "limit": 500}}"#));
    v.get("rows").and_then(Value::as_array).cloned().unwrap_or_default()
}

fn by_title<'a>(rows: &'a [Value], title: &str) -> &'a Value {
    rows.iter().find(|r| r.get("displayTitle").and_then(Value::as_str) == Some(title)).unwrap_or_else(|| panic!("no row {title}"))
}

fn f(v: &Value, k: &str) -> Option<f64> {
    v.get(k).and_then(Value::as_f64)
}

fn s<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str)
}

fn key_name(v: &Value) -> Option<String> {
    v.get("djKey").and_then(Value::as_u64).map(|k| format_key(k as u8, Notation::Musical, true))
}

fn id(v: &Value) -> i64 {
    v.get("id").and_then(Value::as_i64).unwrap()
}

/// A synthetic track as a 16-bit WAV, with optional ID3 tags.
fn write(p: &Path, opts: SynthOptions, tags: Option<TagSet>) {
    let audio = quantize16(&render(&opts));
    let mut w = PcmWriter::create(p, PcmContainer::Wav, opts.sample_rate, 2, 16, tags.map(|t| build(&t)), None).unwrap();
    w.write(&audio).unwrap();
    w.finish().unwrap();
}

fn synth(bpm: f64, transpose: f64, major: bool) -> SynthOptions {
    SynthOptions { seconds: 30.0, bpm, transpose, major, variant: if major { 1 } else { 0 }, ..Default::default() }
}

#[test]
fn detection_tags_edits_filters_and_the_bpm_range() {
    let e = env("detect");
    std::fs::create_dir_all(e.music.join("House")).unwrap();
    std::fs::create_dir_all(e.music.join("Drum & Bass/Liquid")).unwrap();
    write(&e.music.join("House/Groove.wav"), synth(124.0, 0.0, false), None);
    write(&e.music.join("Drum & Bass/Liquid/Roller.wav"), synth(174.0, 5.0, false), None);
    // Tags name a genre and a (wrong) key; detection still wins for the key.
    let tagged = TagSet { title: Some("Tagged".into()), genre: Some("Disco".into()), key: Some("2B".into()), bpm: Some(120.0), ..Default::default() };
    write(&e.music.join("Tagged.wav"), synth(120.0, 3.0, true), Some(tagged));
    let app = open(&e);
    import(&app, &e.music);

    let all = rows(&app, "null", "title");
    assert_eq!(all.len(), 3);
    let groove = by_title(&all, "Groove");
    assert!((f(groove, "djBpm").unwrap() - 124.0).abs() < 0.05, "{}", groove.to_string_compact());
    assert_eq!(s(groove, "djBpmSource"), Some("detected"));
    assert_eq!(key_name(groove).as_deref(), Some("Am"));
    assert_eq!(s(groove, "djKeySource"), Some("detected"));
    assert_eq!(groove.get("djAnalyzed").and_then(Value::as_bool), Some(true));
    assert!(groove.get("djGenre").is_none_or(Value::is_null), "no genre anywhere yet");
    let roller = by_title(&all, "Roller");
    assert!((f(roller, "djBpm").unwrap() - 174.0).abs() < 0.05);
    // D minor, or its relative F major (the same Camelot number, 7).
    let roller_key = roller.get("djKey").and_then(Value::as_u64).unwrap() as u8;
    assert_eq!(sonora_engine::analysis::key::camelot(roller_key).0, 7, "{:?}", key_name(roller));
    let tagged = by_title(&all, "Tagged");
    assert_eq!(s(tagged, "djGenre"), Some("Disco"));
    assert_eq!(s(tagged, "djGenreSource"), Some("tag"));
    assert_eq!(key_name(tagged).as_deref(), Some("Eb"), "detected beats the tag");
    assert!((f(tagged, "djBpm").unwrap() - 120.0).abs() < 0.05);

    // Folder genres, then the BPM range.
    call(&app, "settings.update", r#"{"values": {"genreFromFolders": "top"}}"#);
    let all = rows(&app, "null", "title");
    assert_eq!(s(by_title(&all, "Groove"), "djGenre"), Some("House"));
    assert_eq!(s(by_title(&all, "Roller"), "djGenre"), Some("Drum & Bass"));
    assert_eq!(s(by_title(&all, "Roller"), "djGenreSource"), Some("folder"));
    assert_eq!(s(by_title(&all, "Tagged"), "djGenre"), Some("Disco"), "tags beat folders");
    call(&app, "settings.update", r#"{"values": {"genreFromFolders": "parent", "bpmRange": [60, 120]}}"#);
    let all = rows(&app, "null", "title");
    assert_eq!(s(by_title(&all, "Roller"), "djGenre"), Some("Liquid"));
    assert!((f(by_title(&all, "Roller"), "djBpm").unwrap() - 87.0).abs() < 0.05, "174 shown as 87 in a 60–120 range");
    assert!((f(by_title(&all, "Groove"), "djBpm").unwrap() - 62.0).abs() < 0.05);
    call(&app, "settings.update", r#"{"values": {"bpmRange": [70, 180]}}"#);
    assert_eq!(call_err(&app, "settings.update", r#"{"values": {"bpmRange": [100, 150]}}"#), "invalid_params");

    // Filters and sorting.
    let am = parse_key("Am").unwrap();
    assert_eq!(rows(&app, &format!(r#"{{"keys": [{am}]}}"#), "title").len(), 1);
    assert_eq!(rows(&app, r#"{"bpmMin": 118, "bpmMax": 126}"#, "title").len(), 2);
    assert_eq!(rows(&app, r#"{"bpmMin": 85, "bpmMax": 90}"#, "title").len(), 0);
    assert_eq!(rows(&app, r#"{"bpmMin": 85, "bpmMax": 90, "bpmHalfDouble": true}"#, "title").len(), 1, "174 is double 87");
    assert_eq!(rows(&app, r#"{"genres": ["house"]}"#, "title").len(), 1, "Groove's own folder is House");
    assert_eq!(rows(&app, r#"{"genres": ["drum & bass"]}"#, "title").len(), 0, "Roller's own folder is Liquid");
    assert_eq!(rows(&app, r#"{"genres": ["disco"]}"#, "title").len(), 1, "case-insensitive");
    let by_bpm: Vec<String> = rows(&app, "null", "bpm").iter().map(|r| s(r, "displayTitle").unwrap().to_string()).collect();
    assert_eq!(by_bpm, ["Tagged", "Groove", "Roller"]);
    let facets = call(&app, "library.facets", "");
    assert!(facets.get("genres").and_then(Value::as_array).is_some_and(|g| g.len() == 3));
    assert_eq!(facets.get("djPending").and_then(Value::as_i64), Some(0));

    // Edits: set, halve, clear; files on disk never change.
    let before = std::fs::read(e.music.join("House/Groove.wav")).unwrap();
    let gid = id(by_title(&rows(&app, "null", "title"), "Groove"));
    let rid = id(by_title(&rows(&app, "null", "title"), "Roller"));
    call(&app, "tracks.editDj", &format!(r#"{{"ids": [{gid}], "genre": "Deep House", "key": "9A", "bpm": 123.5}}"#));
    call(&app, "tracks.editDj", &format!(r#"{{"ids": [{rid}], "bpmFactor": 0.5}}"#));
    let all = rows(&app, "null", "title");
    let groove = by_title(&all, "Groove");
    assert_eq!((s(groove, "djGenre"), s(groove, "djGenreSource")), (Some("Deep House"), Some("edit")));
    assert_eq!(key_name(groove).as_deref(), Some("Em"));
    assert_eq!((f(groove, "djBpm"), s(groove, "djBpmSource")), (Some(123.5), Some("edit")));
    assert!((f(by_title(&all, "Roller"), "djBpm").unwrap() - 87.0).abs() < 0.05);
    assert_eq!(rows(&app, r#"{"search": "deep"}"#, "title").len(), 1, "search finds edited genres");
    call(&app, "tracks.editDj", &format!(r#"{{"ids": [{gid}, {rid}], "reset": true}}"#));
    let all = rows(&app, "null", "title");
    assert_eq!(key_name(by_title(&all, "Groove")).as_deref(), Some("Am"));
    assert!((f(by_title(&all, "Roller"), "djBpm").unwrap() - 174.0).abs() < 0.05);
    assert_eq!(call_err(&app, "tracks.editDj", &format!(r#"{{"ids": [{gid}]}}"#)), "invalid_params");
    assert_eq!(call_err(&app, "tracks.editDj", &format!(r#"{{"ids": [{gid}], "key": "H"}}"#)), "invalid_params");
    assert_eq!(std::fs::read(e.music.join("House/Groove.wav")).unwrap(), before);

    // Edits survive a reopen.
    call(&app, "tracks.editDj", &format!(r#"{{"ids": [{gid}], "genre": "Kept"}}"#));
    app.shutdown();
    drop(app);
    let app = open(&e);
    assert_eq!(s(by_title(&rows(&app, "null", "title"), "Groove"), "djGenre"), Some("Kept"));
}

#[test]
fn upgraded_libraries_fill_in_tags_and_offer_detection() {
    let e = env("upgrade");
    write(&e.music.join("Old.wav"), synth(128.0, 0.0, false), Some(TagSet { title: Some("Old".into()), key: Some("Fm".into()), bpm: Some(127.0), genre: Some("Techno".into()), ..Default::default() }));
    let app = open(&e);
    import(&app, &e.music);
    let tid = id(&rows(&app, "null", "title")[0]);
    // Make it look like an analysis from before tempo and key detection.
    {
        let c = app.db.write();
        c.execute_batch(&format!(
            "UPDATE tracks SET dj_version = NULL, bpm_detected = NULL, key_detected = NULL, tempo_candidates = NULL, dj_bpm = NULL, dj_key = NULL, dj_genre = NULL WHERE id = {tid}; \
             INSERT OR REPLACE INTO meta (key, value) VALUES ('dj_refresh', '1');"
        ))
        .unwrap();
    }
    app.shutdown();
    drop(app);
    let app = open(&e);
    let r = rows(&app, "null", "title")[0].clone();
    assert_eq!((f(&r, "djBpm"), s(&r, "djBpmSource")), (Some(127.0), Some("tag")));
    assert_eq!(key_name(&r).as_deref(), Some("Fm"));
    assert_eq!(s(&r, "djGenre"), Some("Techno"));
    assert_eq!(call(&app, "library.facets", "").get("djPending").and_then(Value::as_i64), Some(1));
    let q = call(&app, "tracks.analyzeDj", "{}");
    assert_eq!(q.get("queued").and_then(Value::as_i64), Some(1));
    wait_idle(&app);
    let r = rows(&app, "null", "title")[0].clone();
    assert_eq!(s(&r, "djBpmSource"), Some("detected"));
    assert!((f(&r, "djBpm").unwrap() - 128.0).abs() < 0.05);
    assert_eq!(call(&app, "library.facets", "").get("djPending").and_then(Value::as_i64), Some(0));
}

fn wait_export(app: &Arc<App>) -> Value {
    wait("the export", || call(app, "djexport.status", "").get("running").and_then(Value::as_bool) == Some(false));
    call(app, "djexport.status", "").get("last").cloned().unwrap()
}

fn hash(p: &Path) -> String {
    sonora_engine::pipeline::hash_file(p, &std::sync::atomic::AtomicBool::new(false)).unwrap()
}

#[test]
fn dj_export_writes_tagged_copies_and_a_rekordbox_playlist() {
    let e = env("export");
    write(&e.music.join("Groove.wav"), synth(124.0, 0.0, false), Some(TagSet { title: Some("Groove".into()), artist: Some("Ana Ruiz".into()), year: Some("2023".into()), ..Default::default() }));
    write(&e.music.join("Sunrise.wav"), synth(122.0, 3.0, true), Some(TagSet { title: Some("Sunrise".into()), artist: Some("Ana Ruiz".into()), ..Default::default() }));
    let originals: Vec<(PathBuf, String)> = ["Groove.wav", "Sunrise.wav"].iter().map(|n| (e.music.join(n), hash(&e.music.join(n)))).collect();
    let app = open(&e);
    import(&app, &e.music);
    let all = rows(&app, "null", "title");
    let ids: Vec<i64> = all.iter().map(id).collect();
    let gid = id(by_title(&all, "Groove"));
    call(&app, "tracks.editDj", &format!(r#"{{"ids": [{gid}], "genre": "Deep House"}}"#));

    let plan = call(&app, "djexport.plan", &format!(r#"{{"ids": {ids:?}, "format": "aiff"}}"#));
    assert_eq!(plan.get("exported").and_then(Value::as_i64), Some(2));
    let names: Vec<&str> = plan.get("items").and_then(Value::as_array).unwrap().iter().map(|i| s(i, "fileName").unwrap()).collect();
    assert!(names.contains(&"Ana Ruiz - Groove.aiff"), "{names:?}");
    assert_eq!(call_err(&app, "djexport.start", &format!(r#"{{"ids": {ids:?}, "folder": "relative/dir"}}"#)), "invalid_params");
    assert_eq!(call_err(&app, "djexport.start", &format!(r#"{{"ids": {ids:?}, "folder": "{}"}}"#, e.out.join("nope").display())), "not_found");

    let start = |format: &str, notation: &str| {
        let p = sonora_json::obj! {
            "ids" => ids.clone(), "folder" => e.out.to_string_lossy().to_string(), "format" => format,
            "keyNotation" => notation, "playlistName" => "Friday set",
        };
        call(&app, "djexport.start", &p.to_string_compact());
        wait_export(&app)
    };
    let report = start("aiff", "camelot");
    assert_eq!(report.get("written").and_then(Value::as_array).map(Vec::len), Some(2), "{}", report.to_string_compact());
    assert_eq!(report.get("failed").and_then(Value::as_array).map(Vec::len), Some(0));
    let groove = e.out.join("Ana Ruiz - Groove.aiff");
    let facts = sonora_engine::meta::probe_file(&groove, &sonora_engine::decode::ToolPaths::none()).unwrap();
    assert_eq!((facts.container_name.as_str(), facts.sample_rate, facts.bit_depth), ("AIFF", 44_100, Some(16)));
    assert_eq!(facts.tags.genre.as_deref(), Some("Deep House"));
    assert_eq!(facts.tags.key.as_deref(), Some("8A"));
    assert_eq!(facts.tags.bpm, Some(124.0));
    assert_eq!(facts.tags.artist.as_deref(), Some("Ana Ruiz"));
    assert_eq!(facts.tags.date.as_deref(), Some("2023"));
    let xml_path = PathBuf::from(s(&report, "rekordboxXml").unwrap());
    let xml = std::fs::read_to_string(&xml_path).unwrap();
    assert!(xml.contains("<NODE Name=\"Friday set\""), "{xml}");
    assert!(xml.contains("Tonality=\"8A\""));
    assert!(xml.contains("Genre=\"Deep House\""));
    assert!(xml.contains("Ana%20Ruiz%20-%20Groove.aiff"), "{xml}");
    assert!(xml.contains("Kind=\"AIFF File\""));
    // Nothing is ever overwritten: a second export gets new names.
    let report = start("aiff", "musical");
    assert_eq!(report.get("written").and_then(Value::as_array).map(Vec::len), Some(2));
    assert!(e.out.join("Ana Ruiz - Groove (2).aiff").exists());
    assert!(e.out.join("rekordbox (2).xml").exists());
    let again = sonora_engine::meta::probe_file(&e.out.join("Ana Ruiz - Groove (2).aiff"), &sonora_engine::decode::ToolPaths::none()).unwrap();
    assert_eq!(again.tags.key.as_deref(), Some("Am"));
    let leftovers: Vec<_> = std::fs::read_dir(&e.out).unwrap().flatten().filter(|d| d.file_name().to_string_lossy().ends_with(".sonora-part")).collect();
    assert!(leftovers.is_empty(), "no temporary files left behind");
    for (p, h) in &originals {
        assert_eq!(&hash(p), h, "{} must not change", p.display());
    }
    // WAV too.
    let report = start("wav", "openkey");
    assert_eq!(report.get("written").and_then(Value::as_array).map(Vec::len), Some(2));
    let wav = sonora_engine::meta::probe_file(&e.out.join("Ana Ruiz - Groove.wav"), &sonora_engine::decode::ToolPaths::none()).unwrap();
    assert_eq!(wav.tags.key.as_deref(), Some("1m"));
}

#[test]
fn dj_export_prefers_the_best_copy_and_encodes_mp3() {
    let tools = sonora_engine::decode::ToolPaths::discover(&[]);
    let Some(ffmpeg) = tools.ffmpeg.clone() else {
        eprintln!("FFmpeg not found: skipping");
        return;
    };
    if !sonora_engine::export::convert::has_mp3_encoder(&tools) {
        eprintln!("FFmpeg cannot encode MP3: skipping");
        return;
    }
    let e = env("bestcopy");
    let wav = e.music.join("Master.wav");
    write(&wav, SynthOptions { seconds: 40.0, ..Default::default() }, Some(TagSet { title: Some("Master".into()), ..Default::default() }));
    let mp3 = e.music.join("Master 128.mp3");
    let ok = std::process::Command::new(&ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(&wav)
        .args(["-c:a", "libmp3lame", "-b:a", "128k"])
        .arg(&mp3)
        .status()
        .unwrap()
        .success();
    assert!(ok);
    let app = open(&e);
    import(&app, &e.music);
    call(&app, "duplicates.scan", "{}");
    wait("duplicates", || call(&app, "duplicates.list", "").get("scanning").and_then(Value::as_bool) == Some(false));
    let all = rows(&app, "null", "title");
    let mp3_row = all.iter().find(|r| s(r, "format") == Some("MP3")).unwrap();
    let wav_row = all.iter().find(|r| s(r, "format") == Some("WAV")).unwrap();
    let plan = call(&app, "djexport.plan", &format!(r#"{{"ids": [{}, {}], "format": "aiff"}}"#, id(mp3_row), id(wav_row)));
    let items = plan.get("items").and_then(Value::as_array).unwrap();
    assert_eq!(items[0].get("betterCopy").and_then(Value::as_bool), Some(true), "{}", plan.to_string_compact());
    assert_eq!(items[0].get("sourceId").and_then(Value::as_i64), Some(id(wav_row)));
    assert_eq!(s(&items[1], "skip"), Some("duplicate"), "the same recording is exported once");
    // Without best-copy, the MP3 is exported as it is and warned about.
    let plan = call(&app, "djexport.plan", &format!(r#"{{"ids": [{}], "format": "aiff", "bestCopy": false}}"#, id(mp3_row)));
    let item = &plan.get("items").and_then(Value::as_array).unwrap()[0];
    assert!(item.get("warnings").and_then(Value::as_array).unwrap().iter().any(|w| s(w, "code") == Some("lossy_source")));
    // MP3 export: the MP3 keeps its frames, the WAV is encoded.
    let p = sonora_json::obj! { "ids" => vec![id(mp3_row), id(wav_row)], "folder" => e.out.to_string_lossy().to_string(), "format" => "mp3", "bestCopy" => false };
    call(&app, "djexport.start", &p.to_string_compact());
    let report = wait_export(&app);
    let written = report.get("written").and_then(Value::as_array).unwrap();
    assert_eq!(written.len(), 2, "{}", report.to_string_compact());
    let methods: Vec<&str> = written.iter().map(|w| s(w, "method").unwrap()).collect();
    assert!(methods.contains(&"copied") && methods.contains(&"encoded"), "{methods:?}");
}
