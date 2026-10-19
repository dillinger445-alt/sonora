//! SONORA analysis engine.

pub mod analysis;
pub mod assess;
pub mod decode;
pub mod dsp;
pub mod error;
pub mod export;
pub mod hash;
pub mod md5;
pub mod meta;
pub mod pipeline;
pub mod synth;
pub mod util;

pub use error::AnalysisError;
pub use pipeline::{analyze_file, AnalyzeOptions, FileAnalysis};
