//! Error types for decoding and analysis.

use std::fmt;

/// Failure modes of analysing a single file. Each variant maps to a stable
/// machine-readable [`code`](AnalysisError::code) used by the database and UI.
#[derive(Debug, Clone, PartialEq)]
pub enum AnalysisError {
    /// The file could not be opened or read.
    Io(String),
    /// The content is not a supported/recognised audio format.
    Unsupported(String),
    /// The file is recognised but structurally damaged beyond decoding.
    Corrupt(String),
    /// The file has no decodable audio stream.
    NoAudio,
    /// A required external decoder (FFmpeg) is not available.
    DecoderUnavailable(String),
    /// The audio exceeds the configured maximum duration. `known` when
    /// `seconds` is the file's whole length (declared in its header), not
    /// just how far decoding got before it passed the limit.
    TooLong { seconds: f64, limit: f64, known: bool },
    /// The operation was cancelled by the user.
    Cancelled,
    /// Unexpected internal failure.
    Internal(String),
}

impl AnalysisError {
    pub fn code(&self) -> &'static str {
        match self {
            AnalysisError::Io(_) => "io_error",
            AnalysisError::Unsupported(_) => "unsupported_format",
            AnalysisError::Corrupt(_) => "corrupt_file",
            AnalysisError::NoAudio => "no_audio",
            AnalysisError::DecoderUnavailable(_) => "decoder_unavailable",
            AnalysisError::TooLong { .. } => "too_long",
            AnalysisError::Cancelled => "cancelled",
            AnalysisError::Internal(_) => "internal_error",
        }
    }

    /// Whether retrying the same file could plausibly succeed (e.g. after
    /// installing FFmpeg or reconnecting a drive).
    pub fn retryable(&self) -> bool {
        matches!(self, AnalysisError::Io(_) | AnalysisError::DecoderUnavailable(_) | AnalysisError::Cancelled)
    }
}

impl fmt::Display for AnalysisError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AnalysisError::Io(m) => write!(f, "Could not read file: {m}"),
            AnalysisError::Unsupported(m) => write!(f, "Unsupported format: {m}"),
            AnalysisError::Corrupt(m) => write!(f, "File appears to be corrupt: {m}"),
            AnalysisError::NoAudio => write!(f, "No decodable audio stream found"),
            AnalysisError::DecoderUnavailable(m) => write!(f, "Decoder unavailable: {m}"),
            AnalysisError::TooLong { seconds, limit, known } => {
                // B37: never "2 min long, above the 2 min limit".
                let length = if *seconds < 90.0 { format!("{seconds:.0} s") } else { format!("{:.0} min", seconds / 60.0) };
                let cap = if *limit < 90.0 { format!("{limit:.0} s") } else { format!("{:.0} min", limit / 60.0) };
                if *known && length != cap {
                    write!(f, "Audio is {length} long, above the {cap} analysis limit")
                } else {
                    write!(f, "Audio is longer than the {cap} analysis limit")
                }
            }
            AnalysisError::Cancelled => write!(f, "Analysis cancelled"),
            AnalysisError::Internal(m) => write!(f, "Internal error: {m}"),
        }
    }
}

impl std::error::Error for AnalysisError {}

impl From<std::io::Error> for AnalysisError {
    fn from(e: std::io::Error) -> Self {
        AnalysisError::Io(e.to_string())
    }
}
