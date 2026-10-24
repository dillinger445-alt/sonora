//! Audio decoding.
//!
//! Every decoder implements [`PcmSource`], a pull-based stream of interleaved
//! `f32` frames. Formats are identified from file content ([`sniff`]), never
//! from the extension alone. WAV, AIFF and FLAC are decoded natively; every
//! other format is decoded by an FFmpeg child process ([`ffmpeg`]) that streams
//! a WAV container over a pipe, so the stream format always reflects what the
//! decoder actually produced.

pub mod aiff;
pub mod ffmpeg;
pub mod flac;
pub mod memory;
pub mod pcm;
pub mod sniff;
pub mod tools;
pub mod wav;
pub mod wavstream;

use crate::error::AnalysisError;
use std::path::Path;

/// Something decoders can read from: a file, or audio still arriving (a
/// reader that waits for the bytes it is asked for).
pub trait ReadSeek: std::io::Read + std::io::Seek + Send {}
impl<T: std::io::Read + std::io::Seek + Send> ReadSeek for T {}

pub use sniff::Container;
pub use tools::ToolPaths;

/// What kind of sample data a source produces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleKind {
    /// Integer PCM (bit-exact; effective bit depth can be measured).
    Int,
    /// Floating-point PCM.
    Float,
    /// Output of a lossy decoder (float, may exceed full scale).
    Lossy,
}

impl SampleKind {
    pub fn id(self) -> &'static str {
        match self {
            SampleKind::Int => "int",
            SampleKind::Float => "float",
            SampleKind::Lossy => "lossy",
        }
    }
}

#[derive(Clone, Debug)]
pub struct StreamInfo {
    pub sample_rate: u32,
    pub channels: u16,
    pub kind: SampleKind,
    /// Declared bits per sample for PCM sources.
    pub bits: Option<u16>,
    /// Declared number of frames, when the container states it.
    pub frames_hint: Option<u64>,
    /// Identifier of the decoder in use.
    pub decoder: &'static str,
}

/// Result of an embedded integrity checksum (FLAC stores an MD5 of the
/// decoded audio in its STREAMINFO block).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ChecksumStatus {
    /// The format has no embedded checksum.
    #[default]
    NotApplicable,
    /// The format supports a checksum but the encoder did not store one.
    NotStored,
    /// Decoding stopped early, so the checksum could not be compared.
    NotChecked,
    /// Decoded audio matches the stored checksum.
    Verified,
    /// Decoded audio does not match the stored checksum.
    Mismatch,
}

impl ChecksumStatus {
    pub fn id(self) -> &'static str {
        match self {
            ChecksumStatus::NotApplicable => "not_applicable",
            ChecksumStatus::NotStored => "not_stored",
            ChecksumStatus::NotChecked => "not_checked",
            ChecksumStatus::Verified => "verified",
            ChecksumStatus::Mismatch => "mismatch",
        }
    }
}

/// Diagnostics collected while decoding.
#[derive(Clone, Debug, Default)]
pub struct DecodeStats {
    /// Bitwise OR of all raw integer samples, left-aligned to 32 bits.
    pub int_or_mask: Option<u32>,
    /// Number of decoder error reports (corrupt frames, sync losses...).
    pub decode_errors: u32,
    /// The container declares more audio than the file contains.
    pub truncated: bool,
    /// First few decoder diagnostic messages.
    pub messages: Vec<String>,
    /// Embedded checksum verification (FLAC MD5).
    pub checksum: ChecksumStatus,
}

/// A pull-based decoded audio stream.
pub trait PcmSource: Send {
    fn info(&self) -> &StreamInfo;

    /// Reads up to `max_frames` interleaved frames into `out` (which is
    /// cleared first). Returns the number of frames read; `0` means end of stream.
    fn read(&mut self, out: &mut Vec<f32>, max_frames: usize) -> Result<usize, AnalysisError>;

    /// Finalises the stream and returns decode diagnostics.
    fn finish(&mut self) -> DecodeStats;
}

/// How the caller would like a file decoded.
#[derive(Clone, Copy, Debug, Default)]
pub struct DecodeRequest {
    /// The codec is known to be lossless integer PCM (FLAC, ALAC, WavPack...),
    /// so FFmpeg should emit bit-exact 32-bit integers instead of floats.
    pub lossless_int: bool,
    /// The codec is known to be lossless with floating-point samples (float
    /// WavPack, float PCM in CAF...): FFmpeg emits floats, but they are the
    /// original samples, not the output of a lossy decoder.
    pub lossless_float: bool,
    /// Declared length in frames, used for progress reporting when the
    /// decoder itself cannot know it in advance.
    pub frames_hint: Option<u64>,
    /// The codec is known to be lossy (MP3 or AAC inside a WAV, say): it is
    /// decoded as a lossy source whatever its container, so the decoder's
    /// overshoot above full scale is kept rather than clamped (B35).
    pub lossy: bool,
}

/// Opens the most appropriate decoder for `path`.
///
/// Native decoders are tried first for WAV, AIFF and FLAC. When a native
/// decoder declines the file (compressed WAV payloads, exotic AIFF-C codecs)
/// or fails while FFmpeg is available, decoding falls back to FFmpeg.
pub fn open_source(
    path: &Path,
    container: Container,
    tools: &ToolPaths,
    req: DecodeRequest,
) -> Result<Box<dyn PcmSource>, AnalysisError> {
    let mut native_error: Option<AnalysisError> = None;
    match container {
        Container::Wav => match wav::WavSource::open(path) {
            Ok(src) => return Ok(Box::new(src)),
            Err(wav::OpenError::NotNative(_)) => {}
            Err(wav::OpenError::Failed(e)) => native_error = Some(e),
        },
        Container::Aiff => match aiff::AiffSource::open(path) {
            Ok(src) => return Ok(Box::new(src)),
            Err(aiff::OpenError::NotNative(_)) => {}
            Err(aiff::OpenError::Failed(e)) => native_error = Some(e),
        },
        Container::Flac => match flac::FlacSource::open(path) {
            Ok(src) => return Ok(Box::new(src)),
            Err(flac::OpenError::NotNative(_)) => {}
            Err(flac::OpenError::Failed(e)) => native_error = Some(e),
        },
        _ => {}
    }
    let ffmpeg = match tools.ffmpeg.as_ref() {
        Some(p) => p,
        None => {
            return Err(native_error.unwrap_or_else(|| {
                AnalysisError::DecoderUnavailable(format!(
                    "FFmpeg is required to decode {} audio but was not found",
                    container.display_name()
                ))
            }))
        }
    };
    let lossless_int = !req.lossless_float && !req.lossy && (req.lossless_int || matches!(container, Container::Wav | Container::Aiff | Container::Flac));
    match ffmpeg::FfmpegSource::spawn(ffmpeg, path, lossless_int, req.lossless_float, req.frames_hint) {
        Ok(src) => Ok(Box::new(src)),
        // Prefer the native decoder's diagnosis: it is usually more specific.
        Err(e) => Err(native_error.unwrap_or(match (container, e) {
            // Content we could not identify and FFmpeg could not open either.
            (Container::Unknown, AnalysisError::Corrupt(msg)) | (Container::Unknown, AnalysisError::Unsupported(msg)) => {
                AnalysisError::Unsupported(format!("not a recognised audio file ({})", msg.trim_end_matches('.')))
            }
            (_, other) => other,
        })),
    }
}
