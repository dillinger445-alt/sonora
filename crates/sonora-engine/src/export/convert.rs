//! Turns one audio file into a DJ-ready file: AIFF or WAV (16-bit/44.1 kHz,
//! or up to 24-bit/48 kHz), or MP3 at 320 kbps, with fresh tags.
//!
//! Nothing is "enhanced": the audio is only decoded, resampled when the
//! target rate differs (64-tap windowed-sinc, through FFmpeg), mixed down
//! when it has more than two channels, and requantised with TPDF dither when
//! the output keeps fewer bits than the input carries. 16-bit sources at
//! the target rate come out bit for bit. MP3 sources exported as MP3 keep
//! their audio frames untouched (no second lossy generation).

use super::id3::{build, riff_info, TagSet};
use super::mp3;
use super::writer::{PcmContainer, PcmWriter};
use crate::analysis::CHUNK_FRAMES;
use crate::decode::ffmpeg::FfmpegSource;
use crate::decode::sniff::Container;
use crate::decode::tools::quiet_command;
use crate::decode::{open_source, DecodeRequest, PcmSource, SampleKind, ToolPaths};
use crate::error::AnalysisError;
use crate::meta::{probe_file, FileFacts};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Aiff,
    Wav,
    Mp3,
}

impl Target {
    pub fn from_id(id: &str) -> Option<Target> {
        Some(match id {
            "aiff" => Target::Aiff,
            "wav" => Target::Wav,
            "mp3" => Target::Mp3,
            _ => return None,
        })
    }

    pub fn extension(self) -> &'static str {
        match self {
            Target::Aiff => "aiff",
            Target::Wav => "wav",
            Target::Mp3 => "mp3",
        }
    }
}

/// Resolution of lossless exports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    /// 16-bit, 44.1 kHz: plays on every CDJ, XDJ, controller and program.
    Universal,
    /// The source's resolution, up to 24-bit and 48 kHz (rates above are
    /// brought down to 44.1 or 48 kHz). Some older players refuse 24-bit.
    High,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// Decoded natively and written at the source rate.
    Native,
    /// Decoded (and resampled or mixed down) by FFmpeg.
    Converted,
    /// Encoded to MP3 by FFmpeg (LAME).
    Encoded,
    /// MP3 frames copied unchanged under new tags.
    Copied,
}

pub struct ConvertRequest<'a> {
    pub src: &'a Path,
    pub facts: &'a FileFacts,
    pub target: Target,
    pub quality: Quality,
    pub tags: &'a TagSet,
    pub dest: &'a Path,
    pub tools: &'a ToolPaths,
    pub cancel: &'a AtomicBool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConvertOutcome {
    pub bytes: u64,
    pub sample_rate: u32,
    pub channels: u16,
    /// Bit depth of PCM output.
    pub bits: Option<u16>,
    pub duration: f64,
    pub method: Method,
    pub dithered: bool,
    /// Samples beyond full scale that had to be clamped (lossy sources that overshoot).
    pub clipped_samples: u64,
    /// Decoder complaints about the source.
    pub decode_errors: u32,
}

/// Output sample rate for a source rate.
pub fn output_rate(source: u32, quality: Quality) -> u32 {
    match quality {
        Quality::Universal => 44_100,
        Quality::High => match source {
            44_100 | 48_000 => source,
            r if r > 48_000 && r % 44_100 == 0 => 44_100,
            r if r > 48_000 => 48_000,
            _ => 44_100,
        },
    }
}

/// Whether this FFmpeg can encode MP3 (it needs LAME).
pub fn has_mp3_encoder(tools: &ToolPaths) -> bool {
    let Some(ffmpeg) = &tools.ffmpeg else { return false };
    let mut cmd = quiet_command(ffmpeg);
    cmd.args(["-hide_banner", "-encoders"]);
    crate::decode::tools::run_with_timeout(cmd, Duration::from_secs(10))
        .map(|o| o.success && String::from_utf8_lossy(&o.stdout).contains("libmp3lame"))
        .unwrap_or(false)
}

fn io(e: std::io::Error) -> AnalysisError {
    AnalysisError::Io(e.to_string())
}

/// Converts `req.src` into `req.dest` (created or overwritten: the caller
/// picks a temporary name and moves it into place).
pub fn convert(req: &ConvertRequest, progress: &mut dyn FnMut(f64)) -> Result<ConvertOutcome, AnalysisError> {
    let f = req.facts;
    let outcome = match req.target {
        Target::Mp3 if f.container == Container::Mp3 && f.codec_id == "mp3" => {
            let bytes = mp3::retag(req.src, req.dest, &build(req.tags)).map_err(io)?;
            progress(1.0);
            ConvertOutcome {
                bytes,
                sample_rate: f.sample_rate,
                channels: f.channels,
                bits: None,
                duration: f.header_duration_secs.unwrap_or(0.0),
                method: Method::Copied,
                dithered: false,
                clipped_samples: 0,
                decode_errors: 0,
            }
        }
        Target::Mp3 => encode_mp3(req, progress)?,
        Target::Aiff | Target::Wav => write_pcm(req, progress)?,
    };
    verify(req, &outcome)?;
    Ok(outcome)
}

fn write_pcm(req: &ConvertRequest, progress: &mut dyn FnMut(f64)) -> Result<ConvertOutcome, AnalysisError> {
    let f = req.facts;
    let rate = output_rate(f.sample_rate.max(1), req.quality);
    let lossless = f.lossless == Some(true);
    let source_bits = if lossless && !f.float_samples { f.bit_depth.unwrap_or(16) } else { 32 };
    let bits = match req.quality {
        Quality::Universal => 16,
        Quality::High if lossless && (source_bits > 16 || f.float_samples) => 24,
        Quality::High => 16,
    };
    let frames_hint = f.header_duration_secs.map(|d| (d * rate as f64) as u64);
    let reshaped = f.sample_rate != rate || f.channels > 2;
    let native = matches!(f.container, Container::Wav | Container::Aiff | Container::Flac) && !reshaped;
    let mut src: Box<dyn PcmSource> = if native {
        let req_d = DecodeRequest { lossless_int: lossless && !f.float_samples, lossless_float: lossless && f.float_samples, frames_hint };
        match open_source(req.src, f.container, &ToolPaths::none(), req_d) {
            Ok(s) => s,
            // A WAV or AIFF the native decoders do not handle (compressed payloads).
            Err(_) => open_converted(req, None, None, frames_hint)?,
        }
    } else {
        open_converted(req, (f.sample_rate != rate).then_some(rate), (f.channels > 2).then_some(2), frames_hint)?
    };
    let info = src.info().clone();
    if info.sample_rate != rate || info.channels == 0 || info.channels > 2 {
        return Err(AnalysisError::Unsupported(format!("the decoder delivered {} Hz with {} channels", info.sample_rate, info.channels)));
    }
    // Requantising to fewer bits than the samples carry needs dither;
    // resampling and float or lossy sources always carry more.
    let dithered = info.kind != SampleKind::Int || reshaped || source_bits > bits;
    let container = if req.target == Target::Aiff { PcmContainer::Aiff } else { PcmContainer::Wav };
    let info_list = (container == PcmContainer::Wav).then(|| riff_info(req.tags));
    let mut w = PcmWriter::create(req.dest, container, rate, info.channels, bits, Some(build(req.tags)), info_list).map_err(io)?;
    w.set_dither(dithered);
    let total = frames_hint.unwrap_or(0) as f64;
    let mut buf = Vec::new();
    loop {
        if req.cancel.load(Ordering::Relaxed) {
            return Err(AnalysisError::Cancelled);
        }
        let n = src.read(&mut buf, CHUNK_FRAMES)?;
        if n == 0 {
            break;
        }
        w.write(&buf).map_err(io)?;
        if total > 0.0 {
            progress((w.frames() as f64 / total).min(0.99));
        }
    }
    let stats = src.finish();
    let frames = w.frames();
    let clipped = w.clipped_samples();
    let bytes = w.finish().map_err(io)?;
    progress(1.0);
    Ok(ConvertOutcome {
        bytes,
        sample_rate: rate,
        channels: info.channels,
        bits: Some(bits),
        duration: frames as f64 / rate as f64,
        method: if native { Method::Native } else { Method::Converted },
        dithered,
        clipped_samples: clipped,
        decode_errors: stats.decode_errors,
    })
}

fn open_converted(req: &ConvertRequest, rate: Option<u32>, channels: Option<u16>, frames_hint: Option<u64>) -> Result<Box<dyn PcmSource>, AnalysisError> {
    let ffmpeg = req.tools.ffmpeg.as_ref().ok_or_else(|| {
        AnalysisError::DecoderUnavailable(if rate.is_some() {
            "FFmpeg is needed to change the sample rate but was not found".into()
        } else {
            format!("FFmpeg is needed to decode {} audio but was not found", req.facts.container.display_name())
        })
    })?;
    Ok(Box::new(FfmpegSource::spawn_converted(ffmpeg, req.src, rate, channels, frames_hint)?))
}

fn encode_mp3(req: &ConvertRequest, progress: &mut dyn FnMut(f64)) -> Result<ConvertOutcome, AnalysisError> {
    let f = req.facts;
    let ffmpeg = req.tools.ffmpeg.as_ref().ok_or_else(|| AnalysisError::DecoderUnavailable("FFmpeg is needed to encode MP3 but was not found".into()))?;
    let tmp = req.dest.with_extension("encoding.mp3");
    let mut cmd = quiet_command(ffmpeg);
    cmd.args(["-nostdin", "-hide_banner", "-nostats", "-loglevel", "error", "-protocol_whitelist", "file,pipe", "-i"])
        .arg(format!("file:{}", req.src.display()))
        .args(["-map", "0:a:0", "-vn", "-sn", "-dn", "-map_metadata", "-1"])
        .args(["-c:a", "libmp3lame", "-b:a", "320k", "-ar", "44100"]);
    if f.channels > 2 {
        cmd.args(["-ac", "2"]);
    }
    cmd.args(["-id3v2_version", "0", "-write_id3v1", "0", "-progress", "pipe:1", "-y", "-f", "mp3"])
        .arg(format!("file:{}", tmp.display()))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| AnalysisError::DecoderUnavailable(format!("could not start FFmpeg: {e}")))?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let err_thread = std::thread::spawn(move || {
        let mut lines = Vec::new();
        if let Some(e) = stderr {
            for l in BufReader::new(e).lines().map_while(Result::ok).take(20) {
                lines.push(crate::decode::ffmpeg::clean_message(&l));
            }
        }
        lines
    });
    let duration = f.header_duration_secs.unwrap_or(0.0);
    let (tx, rx) = std::sync::mpsc::channel::<f64>();
    let out_thread = std::thread::spawn(move || {
        if let Some(o) = stdout {
            for l in BufReader::new(o).lines().map_while(Result::ok) {
                if let Some(us) = l.strip_prefix("out_time_us=").and_then(|v| v.trim().parse::<f64>().ok()) {
                    let _ = tx.send(us / 1e6);
                }
            }
        }
    });
    let status = loop {
        while let Ok(t) = rx.try_recv() {
            if duration > 0.0 {
                progress((t / duration).clamp(0.0, 0.99));
            }
        }
        if req.cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_file(&tmp);
            return Err(AnalysisError::Cancelled);
        }
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(io(e)),
        }
    };
    let _ = out_thread.join();
    let messages = err_thread.join().unwrap_or_default();
    if !status.success() {
        let _ = std::fs::remove_file(&tmp);
        let msg = messages.into_iter().find(|m| !m.is_empty()).unwrap_or_else(|| "FFmpeg could not encode the file".into());
        return Err(if msg.contains("libmp3lame") || msg.to_ascii_lowercase().contains("unknown encoder") {
            AnalysisError::DecoderUnavailable("this FFmpeg cannot encode MP3 (it was built without LAME)".into())
        } else {
            AnalysisError::Unsupported(msg)
        });
    }
    let result = mp3::retag(&tmp, req.dest, &build(req.tags)).map_err(io);
    let _ = std::fs::remove_file(&tmp);
    let bytes = result?;
    progress(1.0);
    Ok(ConvertOutcome {
        bytes,
        sample_rate: 44_100,
        channels: f.channels.clamp(1, 2),
        bits: None,
        duration,
        method: Method::Encoded,
        dithered: false,
        clipped_samples: 0,
        decode_errors: 0,
    })
}

/// Reads the new file back: the right format, rate and channels, and about
/// as long as the source.
fn verify(req: &ConvertRequest, out: &ConvertOutcome) -> Result<(), AnalysisError> {
    let facts = probe_file(req.dest, req.tools).map_err(|e| AnalysisError::Internal(format!("the exported file cannot be read back: {e}")))?;
    let expect = match req.target {
        Target::Aiff => Container::Aiff,
        Target::Wav => Container::Wav,
        Target::Mp3 => Container::Mp3,
    };
    if facts.container != expect || facts.sample_rate != out.sample_rate || facts.channels != out.channels {
        return Err(AnalysisError::Internal(format!(
            "the exported file reads back as {} {} Hz {} channels",
            facts.container_name, facts.sample_rate, facts.channels
        )));
    }
    if let (Some(src), Some(dst)) = (req.facts.header_duration_secs, facts.header_duration_secs) {
        // MP3 frames and resampling add or drop a few milliseconds.
        if (src - dst).abs() > 0.25f64.max(src * 0.002) {
            return Err(AnalysisError::Internal(format!("the exported file lasts {dst:.2} s instead of {src:.2} s")));
        }
    }
    Ok(())
}
