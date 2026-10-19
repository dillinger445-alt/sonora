//! Streaming signal analysis.
//!
//! [`analyze_source`] pulls decoded audio from a [`PcmSource`] in fixed-size
//! chunks and feeds every meter in a single pass, so memory use does not grow
//! with track length (apart from small per-second summaries). It produces the
//! [`Measurements`] record, display data ([`Visuals`]), an acoustic
//! fingerprint and a hash of the decoded audio.

pub mod bandwidth;
pub mod fingerprint;
pub mod levels;
pub mod loudness;
pub mod measure;
pub mod silence;
pub mod spectral;
pub mod stereo;
pub mod waveform;

use crate::decode::{PcmSource, SampleKind};
use crate::dsp::truepeak::TruePeakMeter;
use crate::error::AnalysisError;
use crate::hash::Hash128;
use crate::util::{amp_to_db, base64_encode, round_to};
use measure::{BandShare, ClipMeasure, Measurements, SilenceMeasure, MEASUREMENT_SCHEMA};
use sonora_json::{num, obj, ToJson, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// Frames decoded per processing step (~0.37 s at 44.1 kHz).
pub const CHUNK_FRAMES: usize = 16_384;
/// Channels beyond this are ignored by the analysis (and reported).
pub const MAX_ANALYZED_CHANNELS: usize = 8;
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");
/// Segment length used for bandwidth-over-time analysis.
pub const SEGMENT_SECS: f64 = 10.0;

/// Cancellation and progress plumbing for a running analysis.
pub struct Control<'a> {
    pub cancel: &'a AtomicBool,
    /// Called after each chunk with the decoded fraction (0–1), or a negative
    /// value when the total length is unknown.
    pub progress: &'a mut dyn FnMut(f64),
    /// Files longer than this are rejected (protects bulk scans from e.g.
    /// multi-hour recordings when not wanted). `f64::INFINITY` disables it.
    pub max_duration_secs: f64,
}

/// Display data derived from the measurements (not needed for scoring).
#[derive(Clone, Debug, Default)]
pub struct Visuals {
    pub waveform: waveform::Waveform,
    pub spectrogram: spectral::Spectrogram,
    pub spectrum: Vec<(f32, f32, Option<f32>)>,
    pub loudness_step: f64,
    /// Window length of each loudness value (s); see `LoudnessResult`.
    pub loudness_window: f64,
    pub loudness: Vec<Option<f64>>,
    pub clip_times: Vec<f64>,
    pub over_times: Vec<f64>,
    pub silent_regions: Vec<(f64, f64)>,
}

#[derive(Clone, Debug)]
pub struct SignalAnalysis {
    pub measurements: Measurements,
    pub visuals: Visuals,
    pub fingerprint: Vec<u32>,
    /// 128-bit hash of the decoded samples (plus rate and channel count).
    pub audio_hash: String,
}

fn q_lin(v: f32) -> u8 {
    ((v.clamp(-1.0, 1.0) + 1.0) * 127.5).round() as u8
}

fn q_db(v: f32) -> u8 {
    let db = 20.0 * (v.max(1e-9) as f64).log10();
    (((db + 90.0) / 90.0).clamp(0.0, 1.0) * 255.0).round() as u8
}

impl ToJson for Visuals {
    fn to_json(&self) -> Value {
        let b = &self.waveform.buckets;
        let enc = |f: &dyn Fn(&waveform::Bucket) -> u8| base64_encode(&b.iter().map(f).collect::<Vec<u8>>());
        let sg = &self.spectrogram;
        obj! {
            "waveform" => obj! {
                "bucketSecs" => num(self.waveform.secs_per_bucket, 6),
                "count" => b.len(),
                "min" => enc(&|x| q_lin(x.min)),
                "max" => enc(&|x| q_lin(x.max)),
                "rms" => enc(&|x| q_db(x.rms())),
                "low" => enc(&|x| q_db(x.band_rms().0)),
                "mid" => enc(&|x| q_db(x.band_rms().1)),
                "high" => enc(&|x| q_db(x.band_rms().2)),
            },
            "spectrogram" => obj! {
                "cols" => sg.cols, "rows" => sg.rows, "secsPerCol" => num(sg.secs_per_col, 6),
                "maxHz" => num(sg.max_hz, 1), "dbMin" => spectral::SPECTROGRAM_DB_MIN, "dbMax" => spectral::SPECTROGRAM_DB_MAX,
                "data" => base64_encode(&sg.data),
            },
            "spectrum" => obj! {
                "hz" => Value::Array(self.spectrum.iter().map(|p| num(p.0 as f64, 1)).collect()),
                "mid" => Value::Array(self.spectrum.iter().map(|p| num(p.1 as f64, 1)).collect()),
                "side" => if self.spectrum.iter().any(|p| p.2.is_some()) {
                    Value::Array(self.spectrum.iter().map(|p| p.2.map_or(Value::Null, |v| num(v as f64, 1))).collect())
                } else { Value::Null },
            },
            "loudness" => obj! {
                "step" => num(self.loudness_step, 3),
                "window" => num(self.loudness_window, 1),
                "values" => Value::Array(self.loudness.iter().map(|v| v.map_or(Value::Null, |x| num(x, 1))).collect()),
            },
            "clipTimes" => Value::Array(self.clip_times.iter().map(|&t| num(t, 3)).collect()),
            "overTimes" => Value::Array(self.over_times.iter().map(|&t| num(t, 3)).collect()),
            "silentRegions" => Value::Array(self.silent_regions.iter().map(|&(s, e)| Value::Array(vec![num(s, 2), num(e, 2)])).collect()),
        }
    }
}

fn dbfs_of_rms(ms: f64) -> f64 {
    if ms <= 0.0 {
        -200.0
    } else {
        10.0 * ms.log10()
    }
}

/// Runs every meter over `src` in one pass.
///
/// `declared_duration` (from the container) is used to detect truncation;
/// `declared_bits` is the container's bit depth for effective-bit analysis.
pub fn analyze_source(
    src: &mut dyn PcmSource,
    declared_duration: Option<f64>,
    declared_bits: Option<u16>,
    ctl: &mut Control,
) -> Result<SignalAnalysis, AnalysisError> {
    let started = Instant::now();
    let info = src.info().clone();
    if info.sample_rate == 0 || info.channels == 0 {
        return Err(AnalysisError::NoAudio);
    }
    let fs = info.sample_rate;
    let in_ch = info.channels as usize;
    let ch = in_ch.min(MAX_ANALYZED_CHANNELS);
    let frames_hint = info.frames_hint.or_else(|| declared_duration.map(|d| (d * fs as f64) as u64));

    let mut level = levels::LevelMeter::new(fs, ch, info.kind != SampleKind::Lossy);
    let mut tp = TruePeakMeter::new(fs, ch);
    let mut loud = loudness::LoudnessMeter::new(fs, ch);
    let mut st = stereo::StereoMeter::new(fs, ch);
    let mut sil = silence::SilenceMeter::new(fs, ch);
    let mut spec = spectral::SpectralAnalyzer::new(fs, ch, frames_hint);
    let mut wave = waveform::WaveformBuilder::new(fs, ch, frames_hint);
    let mut hasher = Hash128::new();
    hasher.update(&fs.to_le_bytes());
    hasher.update(&(in_ch as u32).to_le_bytes());

    let mut buf: Vec<f32> = Vec::with_capacity(CHUNK_FRAMES * in_ch);
    let mut reduced: Vec<f32> = Vec::new();
    let mut bytes: Vec<u8> = Vec::with_capacity(CHUNK_FRAMES * in_ch * 4);
    let mut frames: u64 = 0;
    loop {
        if ctl.cancel.load(Ordering::Relaxed) {
            return Err(AnalysisError::Cancelled);
        }
        let n = src.read(&mut buf, CHUNK_FRAMES)?;
        if n == 0 {
            break;
        }
        frames += n as u64;
        let secs = frames as f64 / fs as f64;
        if secs > ctl.max_duration_secs {
            return Err(AnalysisError::TooLong { seconds: secs, limit: ctl.max_duration_secs });
        }
        bytes.clear();
        for &x in &buf {
            // Normalise -0.0 so identical audio always hashes identically.
            let v = if x == 0.0 { 0.0f32 } else { x };
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        hasher.update(&bytes);
        let data: &[f32] = if in_ch > ch {
            reduced.clear();
            for fr in buf.chunks_exact(in_ch) {
                reduced.extend_from_slice(&fr[..ch]);
            }
            &reduced
        } else {
            &buf
        };
        level.process(data);
        tp.process(data);
        loud.process(data);
        st.process(data);
        sil.process(data);
        spec.process(data);
        wave.process(data);
        (ctl.progress)(match frames_hint {
            Some(total) if total > 0 => (frames as f64 / total as f64).min(1.0),
            _ => -1.0,
        });
    }
    let stats = src.finish();
    tp.finish();
    let duration = frames as f64 / fs as f64;
    if frames == 0 {
        // Nothing decoded: report as corrupt when the decoder complained.
        if stats.decode_errors > 0 {
            return Err(AnalysisError::Corrupt(stats.messages.first().cloned().unwrap_or_else(|| "no audio could be decoded".into())));
        }
        return Err(AnalysisError::NoAudio);
    }

    // Levels.
    let clip = level.clip_stats();
    let peaks: Vec<f64> = level.levels.iter().map(|l| amp_to_db(l.peak as f64)).collect();
    let rmss: Vec<f64> = level.levels.iter().map(|l| dbfs_of_rms(l.rms() * l.rms())).collect();
    let total_ms: f64 = level.levels.iter().map(|l| l.sum_sq).sum::<f64>() / level.levels.iter().map(|l| l.count).sum::<u64>().max(1) as f64;
    let peak_lin = level.levels.iter().map(|l| l.peak).fold(0.0, f32::max) as f64;
    let rms_db = dbfs_of_rms(total_ms);
    let peak_db = amp_to_db(peak_lin);
    let clipping = clip.as_ref().map(|cs| {
        let events: u64 = cs.iter().map(|c| c.events).sum();
        let samples: u64 = cs.iter().map(|c| c.samples).sum();
        let total_samples = (frames * ch as u64).max(1);
        ClipMeasure {
            events,
            samples,
            longest_run: cs.iter().map(|c| c.longest_run).max().unwrap_or(0),
            events_per_minute: events as f64 / (duration / 60.0).max(1.0 / 60.0),
            ceiling_dbfs: cs.iter().map(|c| c.ceiling_db).fold(f64::NEG_INFINITY, f64::max),
            clipped_fraction: samples as f64 / total_samples as f64,
        }
    });
    let mut clip_times: Vec<f64> = clip.iter().flatten().flat_map(|c| c.event_times.iter().copied()).collect();
    clip_times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    clip_times.dedup_by(|a, b| (*a - *b).abs() < 0.01);
    let clip_times = crate::util::thin_evenly(clip_times, 300);

    let tp_lin = tp.peak().max(peak_lin as f32) as f64;
    let loud_r = loud.finish(1500);
    let stereo_r = st.finish();
    let silence_r = sil.finish();
    let spec_r = spec.finish();
    let wave_r = wave.finish();

    let bw = bandwidth::measure_bandwidth(&spec_r);
    let steep = bw.edge.as_ref().filter(|e| e.is_steep());
    let top = steep.map(|e| e.hz).unwrap_or(bw.upper_hz.max(1.0));
    let margin = steep.map(|e| (e.transition_hz + 500.0).max(1000.0)).unwrap_or(1000.0);
    let artifacts = bandwidth::measure_artifacts(&spec_r.subbands, top, margin, spec_r.nyquist);
    let segments = bandwidth::measure_segments(&spec_r.hf, SEGMENT_SECS, duration);
    let changes = bandwidth::bandwidth_changes(&segments);
    let noise = bandwidth::measure_noise(&spec_r);

    let effective_bits = match (info.kind, stats.int_or_mask) {
        (SampleKind::Int, Some(mask)) if mask != 0 => Some(32 - mask.trailing_zeros() as u16),
        _ => None,
    };
    let container_bits = declared_bits.or(info.bits);
    let truncated_by_length = declared_duration.is_some_and(|d| d > 0.0 && duration < d - (0.5f64).max(d * 0.01));

    let true_peak_dbtp = amp_to_db(tp_lin);
    let measurements = Measurements {
        schema: MEASUREMENT_SCHEMA,
        engine_version: ENGINE_VERSION.to_string(),
        decoder: info.decoder.to_string(),
        sample_kind: info.kind.id().to_string(),
        sample_rate: fs,
        channels: in_ch as u16,
        analyzed_channels: ch as u16,
        duration,
        declared_duration,
        decode_errors: stats.decode_errors,
        decode_messages: stats.messages.clone(),
        truncated: stats.truncated || truncated_by_length,
        checksum: stats.checksum.id().to_string(),
        container_bits,
        effective_bits,
        peak_dbfs: peak_db,
        channel_peak_dbfs: peaks,
        true_peak_dbtp,
        true_peak_oversampling: tp.factor() as u32,
        tp_over_frames: tp.over_count(),
        tp_over_events: tp.over_event_total(),
        rms_dbfs: rms_db,
        channel_rms_dbfs: rmss,
        crest_db: peak_db - rms_db,
        dc_offset: level.levels.iter().map(|l| l.dc()).collect(),
        decoded_overs: level.levels.iter().map(|l| l.overs).sum(),
        clipping,
        integrated_lufs: loud_r.integrated,
        lra: loud_r.lra,
        max_momentary_lufs: loud_r.max_momentary,
        max_short_term_lufs: loud_r.max_short_term,
        plr_db: loud_r.integrated.map(|i| true_peak_dbtp - i),
        stereo: stereo_r,
        silence: SilenceMeasure {
            leading: silence_r.leading_silence,
            trailing: silence_r.trailing_silence,
            silent_fraction: silence_r.silent_fraction,
            all_silent: silence_r.all_silent,
            all_zero: silence_r.all_zero,
            dropout_count: silence_r.dropout_count,
            dropouts: silence_r.dropouts.iter().take(50).cloned().collect(),
            abrupt_cutoff: silence_r.abrupt_cutoff,
            ending_level_db: silence_r.ending_level_db,
        },
        bandwidth: bw,
        artifacts,
        segments,
        bandwidth_changes: changes,
        noise,
        bands: spec_r
            .bands
            .iter()
            .map(|b| BandShare { id: b.id.to_string(), label: b.label.to_string(), lo_hz: b.lo_hz, hi_hz: b.hi_hz, share: b.share, rel_db: round_to(b.rel_db, 2) })
            .collect(),
        analysis_ms: started.elapsed().as_millis() as u64,
    };
    let visuals = Visuals {
        waveform: wave_r,
        spectrogram: spec_r.spectrogram,
        spectrum: spec_r.display,
        loudness_step: loud_r.timeline_step,
        loudness_window: loud_r.timeline_window,
        loudness: loud_r.timeline,
        clip_times,
        over_times: tp.over_events(),
        silent_regions: silence_r.silent_regions,
    };
    Ok(SignalAnalysis { measurements, visuals, fingerprint: spec_r.fingerprint, audio_hash: hasher.hex() })
}
