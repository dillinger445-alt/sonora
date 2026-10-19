//! `sonora` — command-line access to the SONORA analysis engine.
//!
//! ```text
//! sonora analyze <file> [--json] [--no-hash]   analyse one file
//! sonora measure <files...>                    one-line measurement summary per file
//! sonora fixtures <dir> [--seconds N]          build the test/demo audio corpus
//! sonora version
//! ```
//! FFmpeg is located automatically (SONORA_FFMPEG, PATH, common locations);
//! `--ffmpeg <path>` overrides it.

mod fixtures;

use sonora_engine::analysis::Control;
use sonora_engine::decode::ToolPaths;
use sonora_engine::{analyze_file, AnalyzeOptions};
use sonora_json::ToJson;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;

fn usage() -> ExitCode {
    eprintln!(
        "usage:\n  sonora analyze <file> [--json] [--no-hash] [--ffmpeg PATH]\n  sonora measure <files...> [--ffmpeg PATH]\n  sonora fixtures <dir> [--seconds N] [--ffmpeg PATH]\n  sonora compare <file-a> <file-b>\n  sonora version"
    );
    ExitCode::from(2)
}

fn tools(args: &[String]) -> ToolPaths {
    if let Some(i) = args.iter().position(|a| a == "--ffmpeg") {
        if let Some(p) = args.get(i + 1) {
            std::env::set_var("SONORA_FFMPEG", p);
        }
    }
    ToolPaths::discover(&[])
}

fn analyze_one(path: &Path, opts: &AnalyzeOptions) -> Result<sonora_engine::FileAnalysis, sonora_engine::AnalysisError> {
    let cancel = AtomicBool::new(false);
    let mut progress = |_f: f64| {};
    let mut ctl = Control { cancel: &cancel, progress: &mut progress, max_duration_secs: opts.max_duration_secs };
    analyze_file(path, opts, &mut ctl)
}

fn fmt_opt(v: Option<f64>, d: usize) -> String {
    v.map(|x| format!("{x:.d$}")).unwrap_or_else(|| "-".into())
}

fn cmd_measure(files: &[PathBuf], opts: &AnalyzeOptions) {
    println!(
        "{:<34} {:>8} {:>6} {:>6} {:>6} {:>7} | {:>6} {:>5} {:>5} {:>5} {:>6} {:>5} | {:>6} {:>6} {:>5} | {:>6} {:>5} {:>4} {:>4} | {:>5} {:>5} {:>5}",
        "file", "codec", "kbps", "peak", "tp", "lufs", "edge", "drop", "trns", "cons", "p99", "hfc", "upper", "holes", "sfm", "sideEd", "ultra", "bits", "clip", "ovr", "drops", "chg"
    );
    for f in files {
        let name: String = f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default().chars().take(34).collect();
        match analyze_one(f, opts) {
            Err(e) => println!("{name:<34} ERROR {} ({})", e, e.code()),
            Ok(a) => {
                let m = &a.signal.measurements;
                let e = m.bandwidth.edge.as_ref();
                println!(
                    "{:<34} {:>8} {:>6} {:>6.2} {:>6.2} {:>7} | {:>6} {:>5} {:>5} {:>5} {:>6} {:>5.2} | {:>6.0} {:>6} {:>5} | {:>6} {:>5} {:>4} {:>4} | {:>5} {:>5} {:>5}",
                    name,
                    a.facts.codec_id.chars().take(8).collect::<String>(),
                    fmt_opt(a.facts.bitrate_kbps, 0),
                    m.peak_dbfs,
                    m.true_peak_dbtp,
                    fmt_opt(m.integrated_lufs, 1),
                    e.map(|e| format!("{:.0}", e.hz)).unwrap_or("-".into()),
                    e.map(|e| format!("{:.0}", e.drop_db)).unwrap_or("-".into()),
                    e.map(|e| format!("{:.0}", e.transition_hz)).unwrap_or("-".into()),
                    fmt_opt(m.bandwidth.ceiling_consistency, 2),
                    fmt_opt(m.bandwidth.breakthrough_p99_db, 0),
                    m.bandwidth.hf_content,
                    m.bandwidth.upper_hz,
                    fmt_opt(m.artifacts.top_hole_ratio, 3),
                    fmt_opt(m.artifacts.sfm, 2),
                    m.bandwidth.side_edge.as_ref().map(|e| format!("{:.0}", e.hz)).unwrap_or("-".into()),
                    fmt_opt(m.bandwidth.ultrasonic_rel_db, 0),
                    m.effective_bits.map(|b| b.to_string()).unwrap_or("-".into()),
                    m.clipping.as_ref().map(|c| c.events.to_string()).unwrap_or("-".into()),
                    m.decoded_overs,
                    m.silence.dropout_count,
                    m.bandwidth_changes.len(),
                );
            }
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first() else { return usage() };
    let positional: Vec<&String> = {
        let mut v = Vec::new();
        let mut skip = false;
        for a in args.iter().skip(1) {
            if skip {
                skip = false;
                continue;
            }
            if a == "--ffmpeg" || a == "--seconds" {
                skip = true;
                continue;
            }
            if !a.starts_with("--") {
                v.push(a);
            }
        }
        v
    };
    match cmd.as_str() {
        "version" | "--version" | "-V" => {
            println!("sonora {} (engine {}, model {})", env!("CARGO_PKG_VERSION"), sonora_engine::analysis::ENGINE_VERSION, sonora_engine::assess::SCORE_MODEL_VERSION);
            let t = tools(&args);
            println!("ffmpeg: {}", t.ffmpeg_version.unwrap_or_else(|| "not found".into()));
            ExitCode::SUCCESS
        }
        "analyze" => {
            let Some(file) = positional.first() else { return usage() };
            let opts = AnalyzeOptions { tools: tools(&args), hash_file: !args.iter().any(|a| a == "--no-hash"), ..Default::default() };
            match analyze_one(Path::new(file), &opts) {
                Ok(a) => {
                    if args.iter().any(|a| a == "--json") {
                        println!("{}", a.to_json().to_string_pretty());
                    } else {
                        let s = &a.assessment;
                        println!("{}  [{} · {} Hz · {:.1} s]", a.facts.file_name, a.facts.codec_name, a.facts.sample_rate, a.signal.measurements.duration);
                        println!("overall {:.0}  dj {:.0}  listening {:.0}  confidence {} ({:.2})", s.scores.overall, s.scores.dj, s.scores.listening, s.confidence_label, s.confidence);
                        println!("verdict: {}", s.verdict);
                        for f in &s.findings {
                            println!("  [{}/{}] {} — {}", f.severity.id(), f.evidence.id(), f.title, f.detail);
                        }
                    }
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("error ({}): {}", e.code(), e);
                    ExitCode::FAILURE
                }
            }
        }
        "measure" => {
            if positional.is_empty() {
                return usage();
            }
            let opts = AnalyzeOptions { tools: tools(&args), hash_file: false, ..Default::default() };
            let files: Vec<PathBuf> = positional.iter().map(PathBuf::from).collect();
            cmd_measure(&files, &opts);
            ExitCode::SUCCESS
        }
        "compare" => {
            if positional.len() != 2 {
                return usage();
            }
            let opts = AnalyzeOptions { tools: tools(&args), hash_file: false, ..Default::default() };
            let (a, b) = match (analyze_one(Path::new(positional[0]), &opts), analyze_one(Path::new(positional[1]), &opts)) {
                (Ok(a), Ok(b)) => (a, b),
                (Err(e), _) | (_, Err(e)) => {
                    eprintln!("error: {e}");
                    return ExitCode::FAILURE;
                }
            };
            let (fa, fb) = (&a.signal.fingerprint, &b.signal.fingerprint);
            println!("frames: {} vs {}", fa.len(), fb.len());
            println!("audio hash equal: {}", a.signal.audio_hash == b.signal.audio_hash);
            if let Some(c) = sonora_engine::analysis::fingerprint::best_near(fa, fb, 0, 50) {
                println!("best offset {:+.1} s  BER {:.3}  matched {:.1} s  longest {:.1} s", c.offset_secs(), c.ber, c.matched_secs(), c.longest_run_secs());
            }
            ExitCode::SUCCESS
        }
        "fixtures" => {
            let Some(dir) = positional.first() else { return usage() };
            let seconds = args
                .iter()
                .position(|a| a == "--seconds")
                .and_then(|i| args.get(i + 1))
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(45.0)
                .clamp(20.0, 600.0);
            let t = tools(&args);
            match fixtures::generate(Path::new(dir), seconds, t.ffmpeg.as_deref()) {
                Ok(g) => {
                    println!("wrote {} files to {dir}", g.manifest.len());
                    for s in g.skipped {
                        println!("skipped: {s}");
                    }
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        _ => usage(),
    }
}
