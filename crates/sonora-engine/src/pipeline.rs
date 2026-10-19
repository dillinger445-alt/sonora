//! File-level pipeline: inspect → hash → decode → measure → assess.

use crate::analysis::{analyze_source, Control, SignalAnalysis};
use crate::assess::{assess, Assessment, ScoringConfig};
use crate::decode::{open_source, DecodeRequest, ToolPaths};
use crate::error::AnalysisError;
use crate::hash::Hash128;
use crate::meta::{probe_file, FileFacts};
use std::io::Read;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::Instant;

#[derive(Clone, Debug)]
pub struct AnalyzeOptions {
    pub tools: ToolPaths,
    /// Reject files longer than this (seconds).
    pub max_duration_secs: f64,
    /// Compute a hash of the complete file (exact-duplicate detection).
    pub hash_file: bool,
    pub scoring: ScoringConfig,
}

impl Default for AnalyzeOptions {
    fn default() -> Self {
        AnalyzeOptions { tools: ToolPaths::none(), max_duration_secs: 4.0 * 3600.0, hash_file: true, scoring: ScoringConfig::default() }
    }
}

#[derive(Clone, Debug)]
pub struct FileAnalysis {
    pub facts: FileFacts,
    pub signal: SignalAnalysis,
    pub assessment: Assessment,
    pub file_hash: Option<String>,
    pub elapsed_ms: u64,
}

/// Hashes the complete file in 1 MiB reads, checking for cancellation.
pub fn hash_file(path: &Path, cancel: &std::sync::atomic::AtomicBool) -> Result<String, AnalysisError> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Hash128::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(AnalysisError::Cancelled);
        }
        let n = match f.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        };
        h.update(&buf[..n]);
    }
    Ok(h.hex())
}

/// Fills in stream facts the container did not state, from the decoder.
fn reconcile(facts: &mut FileFacts, info: &crate::decode::StreamInfo) {
    if facts.sample_rate == 0 {
        facts.sample_rate = info.sample_rate;
    }
    if facts.channels == 0 {
        facts.channels = info.channels;
    }
    if facts.lossless.is_none() {
        facts.lossless = match info.kind {
            crate::decode::SampleKind::Lossy => None,
            _ => Some(true),
        };
    }
}

/// Analyses one file end to end.
pub fn analyze_file(path: &Path, opts: &AnalyzeOptions, ctl: &mut Control) -> Result<FileAnalysis, AnalysisError> {
    let started = Instant::now();
    let mut facts = probe_file(path, &opts.tools)?;
    let file_hash = if opts.hash_file { Some(hash_file(path, ctl.cancel)?) } else { None };
    let frames_hint = match (facts.header_duration_secs, facts.sample_rate) {
        (Some(d), sr) if d > 0.0 && sr > 0 => Some((d * sr as f64) as u64),
        _ => None,
    };
    let lossless = facts.lossless == Some(true);
    let req = DecodeRequest { lossless_int: lossless && !facts.float_samples, lossless_float: lossless && facts.float_samples, frames_hint };
    let mut src = open_source(path, facts.container, &opts.tools, req)?;
    reconcile(&mut facts, src.info());
    let bits = if facts.lossless == Some(true) { facts.bit_depth } else { None };
    let signal = analyze_source(src.as_mut(), facts.header_duration_secs, bits, ctl)?;
    let assessment = assess(&facts, &signal.measurements, &opts.scoring);
    Ok(FileAnalysis { facts, signal, assessment, file_hash, elapsed_ms: started.elapsed().as_millis() as u64 })
}

impl sonora_json::ToJson for FileAnalysis {
    fn to_json(&self) -> sonora_json::Value {
        sonora_json::obj! {
            "facts" => self.facts,
            "measurements" => self.signal.measurements.to_json(),
            "assessment" => self.assessment,
            "visuals" => self.signal.visuals,
            "fileHash" => self.file_hash,
            "audioHash" => self.signal.audio_hash,
            "elapsedMs" => self.elapsed_ms,
        }
    }
}
