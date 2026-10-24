//! FFmpeg-backed decoding and (optional) ffprobe metadata.
//!
//! FFmpeg runs as a child process that writes a WAVE stream to a pipe, so the
//! analysis never holds an entire track in memory and always learns the true
//! output rate and channel count from the decoder itself (important for
//! HE-AAC, whose core rate differs from its output rate, and Opus, which always
//! decodes at 48 kHz). Lossless integer codecs are requested as `s32le`
//! (bit-exact, enabling effective bit-depth measurement); lossy codecs as
//! `f32le` so decoder overshoot above full scale is preserved. Inputs are
//! restricted to the local `file` protocol.

use super::pcm::{convert, PcmEncoding};
use super::tools::{quiet_command, run_with_timeout};
use super::wavstream;
use super::{ChecksumStatus, DecodeStats, PcmSource, SampleKind, StreamInfo};
use crate::error::AnalysisError;
use sonora_json::Value;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Maximum number of decoder messages retained.
const MAX_MESSAGES: usize = 8;

fn input_arg(path: &Path) -> String {
    // The explicit `file:` protocol prevents FFmpeg from interpreting a path
    // like "concat:..." or "http:..." as another protocol.
    format!("file:{}", path.display())
}

/// If FFmpeg produces no output for this long, it is considered hung and killed.
const STALL_TIMEOUT: Duration = Duration::from_secs(45);

struct StderrSummary {
    lines: u32,
    messages: Vec<String>,
}

/// Shared state between the reader and its watchdog thread.
struct Watch {
    child: Mutex<Option<Child>>,
    last_activity_ms: AtomicU64,
    done: AtomicBool,
    stalled: AtomicBool,
    started: Instant,
}

impl Watch {
    fn touch(&self) {
        self.last_activity_ms.store(self.started.elapsed().as_millis() as u64, Ordering::Relaxed);
    }

    fn kill(&self) {
        if let Ok(mut guard) = self.child.lock() {
            if let Some(child) = guard.as_mut() {
                let _ = child.kill();
            }
        }
    }
}

/// Decodes any FFmpeg-supported file by streaming it as a WAVE container
/// through a pipe. Memory use is bounded by the read size.
pub struct FfmpegSource {
    watch: Arc<Watch>,
    stdout: Option<ChildStdout>,
    stderr_thread: Option<JoinHandle<StderrSummary>>,
    watchdog: Option<JoinHandle<()>>,
    info: StreamInfo,
    encoding: PcmEncoding,
    block_align: usize,
    raw: Vec<u8>,
    or_mask: u32,
    eof: bool,
    frames_out: u64,
}

/// Removes FFmpeg's internal context prefixes such as "[mp3float @ 0x5611…] "
/// or "[in#0 @ 0x…] " so messages read cleanly in the interface.
pub fn clean_message(line: &str) -> String {
    let mut s = line.trim();
    while s.starts_with('[') {
        match s.find("] ") {
            Some(end) if s[..end].contains(" @ ") => s = s[end + 2..].trim_start(),
            _ => break,
        }
    }
    let mut out = s.to_string();
    if let Some(first) = out.get(..1) {
        out = first.to_uppercase() + &out[1..];
    }
    out
}

fn classify_failure(messages: &[String]) -> AnalysisError {
    let joined = messages.join(" | ");
    let lower = joined.to_ascii_lowercase();
    let first = messages.first().cloned().unwrap_or_else(|| "FFmpeg could not decode the file".to_string());
    if lower.contains("matches no streams") || lower.contains("does not contain any stream") {
        AnalysisError::NoAudio
    } else if lower.contains("invalid data found")
        || lower.contains("moov atom not found")
        || lower.contains("could not find codec parameters")
        || lower.contains("error while decoding")
        || lower.contains("truncat")
        || lower.contains("end of file")
    {
        AnalysisError::Corrupt(first)
    } else if lower.contains("no such file") || lower.contains("permission denied") {
        AnalysisError::Io(first)
    } else {
        AnalysisError::Unsupported(first)
    }
}

impl FfmpegSource {
    /// Spawns FFmpeg decoding the first audio stream of `path`.
    ///
    /// `lossless_int` requests bit-exact 32-bit integer output (for lossless
    /// codecs, enabling effective bit-depth measurement); otherwise samples
    /// are 32-bit float so decoder overshoot above full scale is preserved.
    /// `lossless_float` marks float output as original samples (a lossless
    /// float codec) rather than lossy-decoder output. The sample rate and
    /// channel count are whatever the decoder produces (no resampling or
    /// remixing is ever requested).
    pub fn spawn(ffmpeg: &Path, path: &Path, lossless_int: bool, lossless_float: bool, frames_hint: Option<u64>) -> Result<Self, AnalysisError> {
        Self::spawn_shaped(ffmpeg, path, lossless_int, lossless_float, frames_hint, None, None)
    }

    /// Like [`FfmpegSource::spawn`], but resampled to `sample_rate` (with a
    /// high-quality 64-tap filter) and/or mixed down to `channels`, as 32-bit
    /// float. Used for exports; the analysis never alters the audio.
    pub fn spawn_converted(ffmpeg: &Path, path: &Path, sample_rate: Option<u32>, channels: Option<u16>, frames_hint: Option<u64>) -> Result<Self, AnalysisError> {
        Self::spawn_shaped(ffmpeg, path, false, true, frames_hint, sample_rate, channels)
    }

    fn spawn_shaped(
        ffmpeg: &Path,
        path: &Path,
        lossless_int: bool,
        lossless_float: bool,
        frames_hint: Option<u64>,
        sample_rate: Option<u32>,
        channels: Option<u16>,
    ) -> Result<Self, AnalysisError> {
        let codec = if lossless_int { "pcm_s32le" } else { "pcm_f32le" };
        let mut cmd = quiet_command(ffmpeg);
        cmd.args(["-nostdin", "-hide_banner", "-nostats", "-loglevel", "error", "-threads", "1"])
            .args(["-protocol_whitelist", "file,pipe"])
            .arg("-i")
            .arg(input_arg(path))
            .args(["-map", "0:a:0", "-vn", "-sn", "-dn", "-map_metadata", "-1", "-fflags", "+bitexact"]);
        if let Some(rate) = sample_rate {
            cmd.arg("-af").arg(format!("aresample={rate}:filter_size=64:phase_shift=10:cutoff=0.97"));
        }
        if let Some(ch) = channels {
            cmd.arg("-ac").arg(ch.to_string());
        }
        cmd.args(["-acodec", codec, "-f", "wav", "pipe:1"]).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .map_err(|e| AnalysisError::DecoderUnavailable(format!("could not start FFmpeg: {e}")))?;
        let mut stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let stderr_thread = stderr.map(|s| {
            std::thread::spawn(move || {
                let mut summary = StderrSummary { lines: 0, messages: Vec::new() };
                let reader = BufReader::new(s);
                for line in reader.split(b'\n').map_while(Result::ok) {
                    let text = String::from_utf8_lossy(&line).trim().to_string();
                    if text.is_empty() {
                        continue;
                    }
                    summary.lines += 1;
                    if summary.messages.len() < MAX_MESSAGES {
                        summary.messages.push(clean_message(&text).chars().take(240).collect());
                    }
                }
                summary
            })
        });
        let watch = Arc::new(Watch {
            child: Mutex::new(Some(child)),
            last_activity_ms: AtomicU64::new(0),
            done: AtomicBool::new(false),
            stalled: AtomicBool::new(false),
            started: Instant::now(),
        });
        let wd = Arc::clone(&watch);
        let watchdog = std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(250));
            if wd.done.load(Ordering::Relaxed) {
                break;
            }
            let idle = idle_ms(wd.started.elapsed().as_millis() as u64, wd.last_activity_ms.load(Ordering::Relaxed));
            if idle > STALL_TIMEOUT.as_millis() as u64 {
                wd.stalled.store(true, Ordering::Relaxed);
                wd.kill();
                break;
            }
        });

        let header = match stdout.as_mut() {
            Some(out) => wavstream::read_header(out),
            None => Ok(None),
        };
        watch.touch();
        let mut src = FfmpegSource {
            watch,
            stdout,
            stderr_thread,
            watchdog: Some(watchdog),
            info: StreamInfo {
                sample_rate: 0,
                channels: 0,
                kind: if lossless_int {
                    SampleKind::Int
                } else if lossless_float {
                    SampleKind::Float
                } else {
                    SampleKind::Lossy
                },
                bits: None,
                frames_hint,
                decoder: "ffmpeg",
            },
            encoding: PcmEncoding::F32Le,
            block_align: 0,
            raw: Vec::new(),
            or_mask: 0,
            eof: false,
            frames_out: 0,
        };
        match header {
            Ok(Some(fmt)) => {
                src.info.sample_rate = fmt.sample_rate;
                src.info.channels = fmt.channels;
                src.encoding = fmt.encoding;
                src.block_align = fmt.block_align;
                if fmt.encoding.is_float() && lossless_int {
                    src.info.kind = SampleKind::Float;
                }
                Ok(src)
            }
            Ok(None) | Err(_) => {
                let stats = src.finish();
                if src.watch.stalled.load(Ordering::Relaxed) {
                    return Err(AnalysisError::Corrupt("the decoder stopped responding".into()));
                }
                Err(classify_failure(&stats.messages))
            }
        }
    }
}

impl PcmSource for FfmpegSource {
    fn info(&self) -> &StreamInfo {
        &self.info
    }

    fn read(&mut self, out: &mut Vec<f32>, max_frames: usize) -> Result<usize, AnalysisError> {
        out.clear();
        if self.eof || self.block_align == 0 {
            return Ok(0);
        }
        let stdout = match self.stdout.as_mut() {
            Some(s) => s,
            None => return Ok(0),
        };
        let want = max_frames.max(1) * self.block_align;
        self.raw.resize(want, 0);
        let mut got = 0;
        while got < want {
            match stdout.read(&mut self.raw[got..]) {
                Ok(0) => {
                    self.eof = true;
                    break;
                }
                Ok(n) => {
                    got += n;
                    self.watch.touch();
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(AnalysisError::Io(format!("reading decoder output: {e}"))),
            }
        }
        let whole = got - got % self.block_align;
        convert(&self.raw[..whole], self.encoding, out, &mut self.or_mask);
        let frames = whole / self.block_align;
        self.frames_out += frames as u64;
        Ok(frames)
    }

    fn finish(&mut self) -> DecodeStats {
        // Closing stdout makes FFmpeg exit promptly if we stopped early.
        let stopped_early = !self.eof;
        drop(self.stdout.take());
        let mut exit_ok = true;
        let child = self.watch.child.lock().ok().and_then(|mut g| g.take());
        if let Some(mut child) = child {
            if stopped_early {
                let _ = child.kill();
            }
            exit_ok = child.wait().map(|s| s.success()).unwrap_or(false) || stopped_early;
        }
        self.watch.done.store(true, Ordering::Relaxed);
        if let Some(w) = self.watchdog.take() {
            let _ = w.join();
        }
        let summary = self
            .stderr_thread
            .take()
            .and_then(|t| t.join().ok())
            .unwrap_or(StderrSummary { lines: 0, messages: Vec::new() });
        let mut errors = summary.lines;
        let mut messages = summary.messages;
        if self.watch.stalled.load(Ordering::Relaxed) {
            errors = errors.max(1);
            messages.push("The decoder stopped responding and was terminated".to_string());
        } else if !exit_ok {
            errors = errors.max(1);
            if messages.is_empty() {
                messages.push("FFmpeg exited with an error".to_string());
            }
        }
        DecodeStats {
            int_or_mask: if self.info.kind == SampleKind::Int { Some(self.or_mask) } else { None },
            decode_errors: errors,
            truncated: false,
            messages,
            checksum: ChecksumStatus::NotApplicable,
        }
    }
}

impl Drop for FfmpegSource {
    fn drop(&mut self) {
        drop(self.stdout.take());
        if let Ok(mut guard) = self.watch.child.lock() {
            if let Some(mut child) = guard.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        self.watch.done.store(true, Ordering::Relaxed);
    }
}

/// Stream-level facts reported by ffprobe for the first audio stream.
#[derive(Clone, Debug, Default)]
pub struct ProbeStream {
    pub codec_name: String,
    pub codec_long_name: String,
    pub profile: Option<String>,
    pub sample_fmt: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub channel_layout: Option<String>,
    pub bits_per_sample: u16,
    pub bits_per_raw_sample: Option<u16>,
    pub bit_rate: Option<f64>,
    pub duration: Option<f64>,
}

#[derive(Clone, Debug, Default)]
pub struct ProbeResult {
    pub format_name: String,
    pub format_long_name: String,
    pub duration: Option<f64>,
    pub bit_rate: Option<f64>,
    /// Merged format- and stream-level tags with lower-case keys.
    pub tags: Vec<(String, String)>,
    pub stream: Option<ProbeStream>,
}

fn num_field(v: &Value, key: &str) -> Option<f64> {
    match v.get(key)? {
        Value::String(s) => s.trim().parse::<f64>().ok(),
        other => other.as_f64(),
    }
    .filter(|x| x.is_finite())
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(|x| x.as_str()).map(|s| s.to_string()).filter(|s| !s.is_empty() && s != "unknown")
}

fn collect_tags(v: Option<&Value>, out: &mut Vec<(String, String)>) {
    if let Some(Value::Object(map)) = v {
        for (k, val) in map.iter() {
            if let Some(s) = val.as_str() {
                let key = k.to_ascii_lowercase();
                if !out.iter().any(|(existing, _)| *existing == key) && !s.trim().is_empty() {
                    out.push((key, s.trim().to_string()));
                }
            }
        }
    }
}

/// Parses ffprobe's `-print_format json -show_format -show_streams` output.
pub fn parse_probe_json(text: &str) -> Result<ProbeResult, AnalysisError> {
    let root = sonora_json::parse(text).map_err(|e| AnalysisError::Internal(format!("ffprobe output: {e}")))?;
    let mut result = ProbeResult::default();
    if let Some(format) = root.get("format") {
        result.format_name = str_field(format, "format_name").unwrap_or_default();
        result.format_long_name = str_field(format, "format_long_name").unwrap_or_default();
        result.duration = num_field(format, "duration");
        result.bit_rate = num_field(format, "bit_rate");
        collect_tags(format.get("tags"), &mut result.tags);
    }
    if let Some(Value::Array(streams)) = root.get("streams") {
        if let Some(s) = streams.iter().find(|s| s.get("codec_type").and_then(|t| t.as_str()) == Some("audio")) {
            result.stream = Some(ProbeStream {
                codec_name: str_field(s, "codec_name").unwrap_or_default(),
                codec_long_name: str_field(s, "codec_long_name").unwrap_or_default(),
                profile: str_field(s, "profile"),
                sample_fmt: str_field(s, "sample_fmt").unwrap_or_default(),
                sample_rate: num_field(s, "sample_rate").unwrap_or(0.0) as u32,
                channels: num_field(s, "channels").unwrap_or(0.0) as u16,
                channel_layout: str_field(s, "channel_layout"),
                bits_per_sample: num_field(s, "bits_per_sample").unwrap_or(0.0) as u16,
                bits_per_raw_sample: num_field(s, "bits_per_raw_sample").map(|b| b as u16).filter(|b| *b > 0),
                bit_rate: num_field(s, "bit_rate"),
                duration: num_field(s, "duration"),
            });
            collect_tags(s.get("tags"), &mut result.tags);
        }
    }
    Ok(result)
}

/// Runs ffprobe on `path`.
pub fn probe(ffprobe: &Path, path: &Path) -> Result<ProbeResult, AnalysisError> {
    let mut cmd = quiet_command(ffprobe);
    cmd.args(["-v", "error", "-hide_banner", "-protocol_whitelist", "file,pipe"])
        .args(["-print_format", "json", "-show_format", "-show_streams"])
        .arg(input_arg(path));
    let out = run_with_timeout(cmd, Duration::from_secs(30))
        .map_err(|e| AnalysisError::DecoderUnavailable(format!("could not start ffprobe: {e}")))?;
    if out.timed_out {
        return Err(AnalysisError::Corrupt("ffprobe timed out reading the file".into()));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    if !out.success && text.trim().is_empty() {
        let err = String::from_utf8_lossy(&out.stderr);
        let first = err.lines().next().unwrap_or("not a recognised media file").trim();
        return Err(AnalysisError::Unsupported(first.chars().take(200).collect()));
    }
    parse_probe_json(&text)
}

/// Convenience used by tests: path of an FFmpeg sibling tool.
pub fn sibling_tool(ffmpeg: &Path, name: &str) -> Option<PathBuf> {
    let p = ffmpeg.parent()?.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    p.is_file().then_some(p)
}

/// Milliseconds since the decoder last did something. The reader can note
/// activity between the two readings, a moment after `now` (B82): that is
/// no idle time, not a wrap-around to a huge value.
fn idle_ms(now: u64, last_activity: u64) -> u64 {
    now.saturating_sub(last_activity)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_noted_just_after_the_clock_was_read_is_not_a_stall() {
        assert_eq!(idle_ms(1000, 1001), 0);
        assert_eq!(idle_ms(5000, 1000), 4000);
    }

    #[test]
    fn cleans_context_prefixes() {
        assert_eq!(clean_message("[mp3float @ 0x563c1e049000] Header missing"), "Header missing");
        assert_eq!(clean_message("[in#0 @ 0x55a3] Error opening input: Invalid data found"), "Error opening input: Invalid data found");
        assert_eq!(clean_message("[flac] keep me"), "[flac] keep me");
    }

    #[test]
    fn parses_probe_output() {
        let text = r#"{
            "streams": [
                {"index":0,"codec_name":"mjpeg","codec_type":"video"},
                {"index":1,"codec_name":"mp3","codec_long_name":"MP3 (MPEG audio layer 3)","codec_type":"audio",
                 "sample_fmt":"fltp","sample_rate":"44100","channels":2,"channel_layout":"stereo",
                 "bits_per_sample":0,"bit_rate":"320000","duration":"215.4",
                 "tags":{"encoder":"LAME3.100"}}
            ],
            "format": {"format_name":"mp3","format_long_name":"MP2/3 (MPEG audio layer 2/3)",
                       "duration":"215.405714","size":"8616228","bit_rate":"320000",
                       "tags":{"title":"Track","ARTIST":"Someone"}}
        }"#;
        let r = parse_probe_json(text).unwrap();
        let s = r.stream.unwrap();
        assert_eq!(s.codec_name, "mp3");
        assert_eq!(s.sample_rate, 44100);
        assert_eq!(s.channels, 2);
        assert_eq!(s.bit_rate, Some(320000.0));
        assert_eq!(r.duration, Some(215.405714));
        assert!(r.tags.contains(&("artist".to_string(), "Someone".to_string())));
        assert!(r.tags.contains(&("encoder".to_string(), "LAME3.100".to_string())));
    }
}
