//! Builds a folder of real audio files with known technical properties:
//! clean masters in several containers, genuine lossy encodes, transcodes
//! from lossy sources, a "fake 320", clipping, dropouts, truncation,
//! upsampling, padding, DC offset, polarity problems, a spliced file,
//! duplicates and broken files.
//!
//! Lossy variants require FFmpeg (with libmp3lame, libopus and libvorbis
//! for the respective files); they are skipped with a note when unavailable.
//! A `manifest.json` lists every file with the properties it was built to have.

use sonora_engine::synth::{quantize16, render, SynthOptions};
use sonora_engine::decode::wav::write_wav;
use sonora_engine::dsp::biquad::{BiquadCoef, BiquadState};
use sonora_json::{obj, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub struct Generated {
    pub manifest: Vec<Value>,
    pub skipped: Vec<String>,
}

fn ffmpeg_run(ffmpeg: &Path, args: &[&str]) -> Result<(), String> {
    let out = Command::new(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("could not run FFmpeg: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).lines().next().unwrap_or("FFmpeg failed").to_string())
    }
}

/// Decodes any file to 16-bit stereo samples via FFmpeg.
fn ffmpeg_decode16(ffmpeg: &Path, input: &Path, tmp: &Path) -> Result<Vec<f32>, String> {
    let wav = tmp.join(format!("decode-{}.wav", input.file_stem().unwrap_or_default().to_string_lossy()));
    ffmpeg_run(ffmpeg, &["-i", &input.to_string_lossy(), "-ac", "2", "-ar", "44100", "-c:a", "pcm_s16le", &wav.to_string_lossy()])?;
    let mut src = sonora_engine::decode::wav::WavSource::open(&wav).map_err(|_| "decode failed".to_string())?;
    let mut all = Vec::new();
    let mut buf = Vec::new();
    use sonora_engine::decode::PcmSource;
    loop {
        let n = src.read(&mut buf, 65536).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        all.extend_from_slice(&buf);
    }
    let _ = std::fs::remove_file(&wav);
    Ok(all)
}

fn entry(file: &str, description: &str, expect: &[&str]) -> Value {
    obj! { "file" => file, "description" => description, "expect" => expect.iter().map(|s| s.to_string()).collect::<Vec<_>>() }
}

pub fn generate(out: &Path, seconds: f64, ffmpeg: Option<&Path>) -> Result<Generated, String> {
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    let tmp = out.join(".tmp");
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let mut manifest = Vec::new();
    let mut skipped = Vec::new();
    let w = |name: &str, bits: u16, s: &[f32]| -> Result<PathBuf, String> {
        let p = out.join(name);
        write_wav(&p, 44_100, 2, bits, false, s).map_err(|e| e.to_string())?;
        Ok(p)
    };

    let base = render(&SynthOptions { seconds, ..Default::default() });
    let clean16 = quantize16(&base);

    // Lossless masters.
    w("clean_master_24bit.wav", 24, &base)?;
    manifest.push(entry("clean_master_24bit.wav", "Clean 24-bit master, full bandwidth", &["lossless", "clean"]));
    let clean_wav = w("clean_master_16bit.wav", 16, &clean16)?;
    manifest.push(entry("clean_master_16bit.wav", "Clean 16-bit master", &["lossless", "clean"]));
    let padded = w("padded_24bit.wav", 24, &clean16)?;
    let _ = padded;
    manifest.push(entry("padded_24bit.wav", "16-bit audio stored in a 24-bit container", &["lossless", "padded_bits"]));

    let mut dc = clean16.clone();
    dc.iter_mut().for_each(|v| *v = (*v * 0.9 + 0.02).clamp(-1.0, 1.0));
    w("dc_offset.wav", 16, &quantize16(&dc))?;
    manifest.push(entry("dc_offset.wav", "Constant +0.02 DC offset", &["lossless", "dc_offset"]));

    let mut inv = clean16.clone();
    inv.iter_mut().skip(1).step_by(2).for_each(|v| *v = -*v);
    w("polarity_inverted.wav", 16, &inv)?;
    manifest.push(entry("polarity_inverted.wav", "Right channel polarity inverted (wiring error)", &["lossless", "phase"]));

    let mut mono = Vec::with_capacity(clean16.len());
    for fr in clean16.chunks_exact(2) {
        let m = (fr[0] + fr[1]) * 0.5;
        mono.push(m);
        mono.push(m);
    }
    let mono_wav = w(".tmp/mono.wav", 16, &quantize16(&mono))?;

    let mut drop = clean16.clone();
    for start in [0.27, 0.55, 0.74] {
        let s = ((seconds * start * 44_100.0) as usize) * 2;
        let len = (0.15 * 44_100.0) as usize * 2;
        drop[s..(s + len).min(clean16.len())].iter_mut().for_each(|v| *v = 0.0);
    }
    w("dropouts.wav", 16, &drop)?;
    manifest.push(entry("dropouts.wav", "Three 150 ms digital-silence gaps inside the music", &["lossless", "dropouts"]));

    let mut tail = clean16.clone();
    let cut = (clean16.len() as f64 * 0.62) as usize & !1;
    tail[cut..].iter_mut().for_each(|v| *v = 0.0);
    w("zero_filled_tail.wav", 16, &tail)?;
    manifest.push(entry("zero_filled_tail.wav", "Audio stops abruptly; the rest is zero-filled (incomplete download)", &["lossless", "cutoff_tail"]));

    // Clipping: drive 4 dB into a hard clip.
    let g = 10f64.powf(4.0 / 20.0) as f32;
    let clipped: Vec<f32> = base.iter().map(|&v| (v * g).clamp(-1.0, 1.0)).collect();
    w("clipped_master.wav", 16, &quantize16(&clipped))?;
    manifest.push(entry("clipped_master.wav", "Hard clipped at 0 dBFS (4 dB of overdrive)", &["lossless", "clipping"]));
    let norm = 10f64.powf(-0.3 / 20.0) as f32;
    let clipped_norm: Vec<f32> = clipped.iter().map(|&v| v * norm).collect();
    let clipped_norm_wav = w(".tmp/clipped_norm.wav", 16, &quantize16(&clipped_norm))?;

    w("silent.wav", 16, &vec![0f32; 44_100 * 2 * 8])?;
    manifest.push(entry("silent.wav", "Eight seconds of digital silence", &["silent"]));

    // Hiss: 3 s of noise alone, then the track at -6 dB with noise underneath.
    let mut rng = 12345u64;
    let mut hiss = Vec::new();
    let noise_amp = 10f64.powf(-50.0 / 20.0) * 3f64.sqrt();
    let mut nz = || {
        rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (((rng >> 11) as f64 / (1u64 << 53) as f64) * 2.0 - 1.0) * noise_amp
    };
    for _ in 0..(3 * 44_100 * 2) {
        hiss.push(nz() as f32);
    }
    for &v in &base {
        hiss.push((v as f64 * 0.5 + nz()) as f32);
    }
    w("noisy_hiss.wav", 16, &quantize16(&hiss))?;
    manifest.push(entry("noisy_hiss.wav", "Broadband hiss at about -50 dBFS, audible in the intro", &["lossless", "noise"]));

    // Dark but natural master: gentle 12 dB/octave low-pass at 7 kHz.
    let c = BiquadCoef::lowpass(44_100.0, 7000.0, std::f64::consts::FRAC_1_SQRT_2);
    let mut sl = BiquadState::default();
    let mut sr = BiquadState::default();
    let dark: Vec<f32> = base
        .chunks_exact(2)
        .flat_map(|fr| [sl.process(&c, fr[0] as f64) as f32, sr.process(&c, fr[1] as f64) as f32])
        .collect();
    let dark_wav = w(".tmp/dark.wav", 16, &quantize16(&dark))?;

    // Brickwall at 16 kHz on a lossless master (16 cascaded sections).
    let bw = BiquadCoef::lowpass(44_100.0, 16_000.0, std::f64::consts::FRAC_1_SQRT_2);
    let mut st_l = vec![BiquadState::default(); 16];
    let mut st_r = vec![BiquadState::default(); 16];
    let brick: Vec<f32> = base
        .chunks_exact(2)
        .flat_map(|fr| {
            let mut l = fr[0] as f64;
            let mut r = fr[1] as f64;
            for s in st_l.iter_mut() {
                l = s.process(&bw, l);
            }
            for s in st_r.iter_mut() {
                r = s.process(&bw, r);
            }
            [l as f32, r as f32]
        })
        .collect();
    let brick_wav = w(".tmp/brick.wav", 16, &quantize16(&brick))?;

    let other = quantize16(&render(&SynthOptions { seconds, seed: 2, variant: 1, bpm: 128.0, ..Default::default() }));
    let other_wav = w(".tmp/other.wav", 16, &other)?;

    // Radio edit: a 25 s excerpt starting 12 s in, with a short fade-out.
    let start = (12.0 * 44_100.0) as usize * 2;
    let len = ((seconds - 14.0).min(25.0) * 44_100.0) as usize * 2;
    let mut edit = clean16[start..(start + len).min(clean16.len())].to_vec();
    let fade = (1.5 * 44_100.0) as usize * 2;
    let el = edit.len();
    for (i, v) in edit.iter_mut().enumerate().skip(el.saturating_sub(fade)) {
        *v *= (el - i) as f32 / fade as f32;
    }
    let edit_wav = w(".tmp/edit.wav", 16, &quantize16(&edit))?;

    // Garbage files.
    let mut junk = Vec::with_capacity(200_000);
    let mut s = 777u64;
    for _ in 0..200_000 {
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        junk.push((s >> 56) as u8);
    }
    std::fs::write(out.join("corrupt.mp3"), &junk).map_err(|e| e.to_string())?;
    manifest.push(entry("corrupt.mp3", "Random bytes with an .mp3 extension", &["error"]));
    std::fs::write(out.join("notes_renamed.wav"), b"These are my set notes, not audio.\n").map_err(|e| e.to_string())?;
    manifest.push(entry("notes_renamed.wav", "A text file renamed to .wav", &["error"]));
    std::fs::write(out.join("empty.flac"), []).map_err(|e| e.to_string())?;
    manifest.push(entry("empty.flac", "A zero-byte file", &["error"]));

    let Some(ff) = ffmpeg else {
        skipped.push("FFmpeg not found: FLAC/AIFF/lossy variants, transcodes, duplicates and the spliced file were skipped".into());
        let _ = std::fs::remove_dir_all(&tmp);
        return Ok(Generated { manifest, skipped });
    };
    let p = |name: &str| out.join(name).to_string_lossy().to_string();
    let src = clean_wav.to_string_lossy().to_string();
    let run = |name: &str, args: Vec<String>, desc: &str, expect: &[&str], manifest: &mut Vec<Value>, skipped: &mut Vec<String>| {
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        match ffmpeg_run(ff, &refs) {
            Ok(()) => manifest.push(entry(name, desc, expect)),
            Err(e) => skipped.push(format!("{name}: {e}")),
        }
    };
    let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<String>>();

    run("clean_master.flac", a(&["-i", &src, "-c:a", "flac", "-compression_level", "8", &p("clean_master.flac")]), "Clean 16-bit FLAC", &["lossless", "clean"], &mut manifest, &mut skipped);
    run("clean_master.aiff", a(&["-i", &src, "-c:a", "pcm_s16be", &p("clean_master.aiff")]), "Clean 16-bit AIFF", &["lossless", "clean"], &mut manifest, &mut skipped);
    run("mp3_320.mp3", a(&["-i", &src, "-c:a", "libmp3lame", "-b:a", "320k", &p("mp3_320.mp3")]), "MP3 320 kbps CBR (LAME)", &["lossy"], &mut manifest, &mut skipped);
    run("mp3_v0.mp3", a(&["-i", &src, "-c:a", "libmp3lame", "-q:a", "0", &p("mp3_v0.mp3")]), "MP3 V0 VBR (LAME)", &["lossy"], &mut manifest, &mut skipped);
    run("mp3_128.mp3", a(&["-i", &src, "-c:a", "libmp3lame", "-b:a", "128k", &p("mp3_128.mp3")]), "MP3 128 kbps CBR (LAME)", &["lossy", "low_bitrate"], &mut manifest, &mut skipped);
    run("aac_256.m4a", a(&["-i", &src, "-c:a", "aac", "-b:a", "256k", &p("aac_256.m4a")]), "AAC-LC 256 kbps (M4A)", &["lossy"], &mut manifest, &mut skipped);
    run("aac_128.m4a", a(&["-i", &src, "-c:a", "aac", "-b:a", "128k", &p("aac_128.m4a")]), "AAC-LC 128 kbps (M4A)", &["lossy"], &mut manifest, &mut skipped);
    run("opus_160.opus", a(&["-i", &src, "-c:a", "libopus", "-b:a", "160k", &p("opus_160.opus")]), "Opus 160 kbps", &["lossy"], &mut manifest, &mut skipped);
    run("vorbis_q6.ogg", a(&["-i", &src, "-c:a", "libvorbis", "-q:a", "6", &p("vorbis_q6.ogg")]), "Ogg Vorbis q6", &["lossy"], &mut manifest, &mut skipped);
    run("alac_16.m4a", a(&["-i", &src, "-c:a", "alac", &p("alac_16.m4a")]), "Apple Lossless 16-bit", &["lossless", "clean"], &mut manifest, &mut skipped);
    run(
        "upsampled_96k.flac",
        a(&["-i", &src, "-ar", "96000", "-sample_fmt", "s32", "-c:a", "flac", &p("upsampled_96k.flac")]),
        "44.1 kHz master upsampled to 96 kHz/24-bit",
        &["lossless", "upsampled"],
        &mut manifest,
        &mut skipped,
    );
    let cn = clipped_norm_wav.to_string_lossy().to_string();
    run("clipped_normalized.flac", a(&["-i", &cn, "-c:a", "flac", &p("clipped_normalized.flac")]), "Hard clipped, then normalised to -0.3 dBFS", &["lossless", "clipping"], &mut manifest, &mut skipped);
    let mw = mono_wav.to_string_lossy().to_string();
    run("mono_as_stereo.flac", a(&["-i", &mw, "-c:a", "flac", &p("mono_as_stereo.flac")]), "Identical left and right channels", &["lossless", "dual_mono"], &mut manifest, &mut skipped);
    let dw = dark_wav.to_string_lossy().to_string();
    run("dark_master.flac", a(&["-i", &dw, "-c:a", "flac", &p("dark_master.flac")]), "Naturally dark master (gentle 12 dB/oct roll-off above 7 kHz)", &["lossless", "clean", "dark"], &mut manifest, &mut skipped);
    let bwp = brick_wav.to_string_lossy().to_string();
    run(
        "brickwall_16k.flac",
        a(&["-i", &bwp, "-c:a", "flac", &p("brickwall_16k.flac")]),
        "Lossless master with a deliberate steep 16 kHz low-pass (ambiguous case)",
        &["lossless", "steep_cutoff"],
        &mut manifest,
        &mut skipped,
    );
    let ow = other_wav.to_string_lossy().to_string();
    run("other_track.flac", a(&["-i", &ow, "-c:a", "flac", &p("other_track.flac")]), "A different track (should match nothing)", &["lossless", "clean"], &mut manifest, &mut skipped);
    let ew = edit_wav.to_string_lossy().to_string();
    run("radio_edit.flac", a(&["-i", &ew, "-c:a", "flac", &p("radio_edit.flac")]), "25 s edit of the clean master", &["lossless", "alternate_version"], &mut manifest, &mut skipped);

    // Transcodes from lossy sources.
    let make_transcode = |from: &str, to: &str, codec_args: &[&str]| -> Result<(), String> {
        let decoded = ffmpeg_decode16(ff, &out.join(from), &tmp)?;
        let t = tmp.join(format!("{to}.wav"));
        write_wav(&t, 44_100, 2, 16, false, &decoded).map_err(|e| e.to_string())?;
        let mut args = vec!["-i".to_string(), t.to_string_lossy().to_string()];
        args.extend(codec_args.iter().map(|s| s.to_string()));
        args.push(out.join(to).to_string_lossy().to_string());
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        ffmpeg_run(ff, &refs)
    };
    for (from, to, codec, desc, expect) in [
        ("mp3_128.mp3", "transcode_mp3_128.flac", vec!["-c:a", "flac"], "FLAC made from a 128 kbps MP3", vec!["lossless_container", "lossy_source"]),
        ("mp3_320.mp3", "transcode_mp3_320.wav", vec!["-c:a", "pcm_s16le"], "WAV made from a 320 kbps MP3", vec!["lossless_container", "lossy_source_hq"]),
        ("aac_128.m4a", "transcode_aac_128.flac", vec!["-c:a", "flac"], "FLAC made from a 128 kbps AAC", vec!["lossless_container", "lossy_source"]),
        ("mp3_128.mp3", "fake_320_from_128.mp3", vec!["-c:a", "libmp3lame", "-b:a", "320k"], "320 kbps MP3 re-encoded from a 128 kbps MP3", vec!["lossy", "fake_bitrate"]),
    ] {
        if !out.join(from).exists() {
            skipped.push(format!("{to}: source {from} missing"));
            continue;
        }
        match make_transcode(from, to, &codec) {
            Ok(()) => manifest.push(entry(to, desc, &expect)),
            Err(e) => skipped.push(format!("{to}: {e}")),
        }
    }

    // Spliced: first half clean, second half from the MP3-128 transcode.
    if out.join("mp3_128.mp3").exists() {
        if let Ok(lossy) = ffmpeg_decode16(ff, &out.join("mp3_128.mp3"), &tmp) {
            let half = (clean16.len() / 2) & !1;
            let mut spliced = clean16[..half].to_vec();
            spliced.extend_from_slice(&lossy[half.min(lossy.len())..]);
            let t = w(".tmp/spliced.wav", 16, &spliced)?;
            run(
                "spliced_sources.flac",
                a(&["-i", &t.to_string_lossy(), "-c:a", "flac", &p("spliced_sources.flac")]),
                "First half from the master, second half from a 128 kbps MP3",
                &["lossless_container", "bandwidth_change"],
                &mut manifest,
                &mut skipped,
            );
        }
    }

    // Truncated FLAC (cut at 60 %).
    if let Ok(bytes) = std::fs::read(out.join("clean_master.flac")) {
        let cut = bytes.len() * 6 / 10;
        if std::fs::write(out.join("truncated.flac"), &bytes[..cut]).is_ok() {
            manifest.push(entry("truncated.flac", "FLAC cut off at 60 % of its size", &["truncated"]));
        }
        // Duplicates in a sub-folder.
        let dupes = out.join("duplicates");
        if std::fs::create_dir_all(&dupes).is_ok() && std::fs::write(dupes.join("clean_master (copy).flac"), &bytes).is_ok() {
            manifest.push(entry("duplicates/clean_master (copy).flac", "Byte-identical copy of clean_master.flac", &["lossless", "clean", "exact_duplicate"]));
        }
        let _ = std::fs::copy(&clean_wav, dupes.join("clean_master (export).wav"));
        manifest.push(entry("duplicates/clean_master (export).wav", "Same audio as clean_master.flac in a WAV container", &["lossless", "clean", "same_audio"]));
    }

    // An MP3 whose extension says FLAC.
    if out.join("mp3_320.mp3").exists() && std::fs::copy(out.join("mp3_320.mp3"), out.join("actually_mp3.flac")).is_ok() {
        manifest.push(entry("actually_mp3.flac", "An MP3 file with a .flac extension", &["lossy", "extension_mismatch"]));
    }

    let _ = std::fs::remove_dir_all(&tmp);
    manifest.sort_by(|a, b| a.get("file").and_then(Value::as_str).cmp(&b.get("file").and_then(Value::as_str)));
    let doc = obj! { "generator" => "sonora-cli fixtures", "seconds" => seconds, "files" => Value::Array(manifest.clone()) };
    std::fs::write(out.join("manifest.json"), doc.to_string_pretty()).map_err(|e| e.to_string())?;
    Ok(Generated { manifest, skipped })
}
