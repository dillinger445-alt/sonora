//! The measurement record: every number the DSP stage produces for a file.
//!
//! Measurements are facts about the decoded signal. They are stored with the
//! analysis so the (cheap, deterministic) assessment and scoring stage can be
//! re-run when the scoring model changes, without decoding audio again.
//! The record round-trips through JSON ([`Measurements::to_json`] /
//! [`Measurements::from_json`]).

use super::bandwidth::{ArtifactMeasure, BandwidthChange, BandwidthMeasure, Edge, NoiseMeasure, SegmentBandwidth};
use super::silence::Dropout;
use super::key::{format_key, KeyResult, Notation};
use super::stereo::StereoResult;
use super::tempo::TempoResult;
use sonora_json::{num, obj, opt_num, ToJson, Value};

/// Version of the measurement schema. Bump when fields change meaning.
pub const MEASUREMENT_SCHEMA: u32 = 2;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClipMeasure {
    /// Flat runs (≥ 3 equal samples) at each channel's ceiling, summed over channels.
    pub events: u64,
    pub samples: u64,
    pub longest_run: u32,
    pub events_per_minute: f64,
    /// Highest channel ceiling (dBFS).
    pub ceiling_dbfs: f64,
    /// Clipped samples as a fraction of all samples.
    pub clipped_fraction: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SilenceMeasure {
    pub leading: f64,
    pub trailing: f64,
    pub silent_fraction: f64,
    pub all_silent: bool,
    pub all_zero: bool,
    pub dropout_count: u64,
    pub dropouts: Vec<Dropout>,
    /// (start, length) of a zero-filled ending that began abruptly.
    pub abrupt_cutoff: Option<(f64, f64)>,
    pub ending_level_db: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BandShare {
    pub id: String,
    pub label: String,
    pub lo_hz: f64,
    pub hi_hz: f64,
    pub share: f64,
    pub rel_db: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Measurements {
    pub schema: u32,
    pub engine_version: String,
    pub decoder: String,
    pub sample_kind: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub analyzed_channels: u16,
    pub duration: f64,
    pub declared_duration: Option<f64>,
    pub decode_errors: u32,
    pub decode_messages: Vec<String>,
    pub truncated: bool,
    pub checksum: String,
    pub container_bits: Option<u16>,
    pub effective_bits: Option<u16>,

    pub peak_dbfs: f64,
    pub channel_peak_dbfs: Vec<f64>,
    pub true_peak_dbtp: f64,
    pub true_peak_oversampling: u32,
    pub tp_over_frames: u64,
    pub tp_over_events: u64,
    pub rms_dbfs: f64,
    pub channel_rms_dbfs: Vec<f64>,
    pub crest_db: f64,
    pub dc_offset: Vec<f64>,
    /// Samples decoded above full scale (float/lossy sources).
    pub decoded_overs: u64,
    /// `None` for lossy sources, where flat-run clipping detection is not meaningful.
    pub clipping: Option<ClipMeasure>,

    pub integrated_lufs: Option<f64>,
    pub lra: Option<f64>,
    pub max_momentary_lufs: Option<f64>,
    pub max_short_term_lufs: Option<f64>,
    /// Peak-to-loudness ratio: true peak minus integrated loudness.
    pub plr_db: Option<f64>,

    pub stereo: Option<StereoResult>,
    pub silence: SilenceMeasure,
    pub bandwidth: BandwidthMeasure,
    pub artifacts: ArtifactMeasure,
    pub segments: Vec<SegmentBandwidth>,
    pub bandwidth_changes: Vec<BandwidthChange>,
    pub noise: NoiseMeasure,
    pub bands: Vec<BandShare>,
    /// Tempo; `None` when there is no regular pulse (or for schema 1 records).
    pub tempo: Option<TempoResult>,
    /// Musical key; `None` without enough tonal material (or for schema 1 records).
    pub key: Option<KeyResult>,
    pub analysis_ms: u64,
}

// ---------------------------------------------------------------- to JSON

fn edge_json(e: &Option<Edge>) -> Value {
    match e {
        None => Value::Null,
        Some(e) => obj! {
            "hz" => num(e.hz, 0), "dropDb" => num(e.drop_db, 1), "edgeDb" => num(e.edge_db, 1),
            "shelfDb" => num(e.shelf_db, 1), "floorDb" => num(e.floor_db, 1),
            "transitionHz" => num(e.transition_hz, 0), "floorStdevDb" => num(e.floor_stdev_db, 2),
        },
    }
}

fn nums(v: &[f64], d: u32) -> Value {
    Value::Array(v.iter().map(|&x| num(x, d)).collect())
}

impl ToJson for Measurements {
    fn to_json(&self) -> Value {
        let m = self;
        obj! {
            "schema" => m.schema,
            "engineVersion" => m.engine_version,
            "decoder" => m.decoder,
            "sampleKind" => m.sample_kind,
            "sampleRate" => m.sample_rate,
            "channels" => m.channels,
            "analyzedChannels" => m.analyzed_channels,
            "duration" => num(m.duration, 3),
            "declaredDuration" => opt_num(m.declared_duration, 3),
            "decodeErrors" => m.decode_errors,
            "decodeMessages" => m.decode_messages,
            "truncated" => m.truncated,
            "checksum" => m.checksum,
            "containerBits" => m.container_bits,
            "effectiveBits" => m.effective_bits,
            "peakDbfs" => num(m.peak_dbfs, 2),
            "channelPeakDbfs" => nums(&m.channel_peak_dbfs, 2),
            "truePeakDbtp" => num(m.true_peak_dbtp, 2),
            "truePeakOversampling" => m.true_peak_oversampling,
            "tpOverFrames" => m.tp_over_frames,
            "tpOverEvents" => m.tp_over_events,
            "rmsDbfs" => num(m.rms_dbfs, 2),
            "channelRmsDbfs" => nums(&m.channel_rms_dbfs, 2),
            "crestDb" => num(m.crest_db, 2),
            "dcOffset" => nums(&m.dc_offset, 6),
            "decodedOvers" => m.decoded_overs,
            "clipping" => match &m.clipping {
                None => Value::Null,
                Some(c) => obj! {
                    "events" => c.events, "samples" => c.samples, "longestRun" => c.longest_run,
                    "eventsPerMinute" => num(c.events_per_minute, 2), "ceilingDbfs" => num(c.ceiling_dbfs, 2),
                    "clippedFraction" => num(c.clipped_fraction, 8),
                },
            },
            "integratedLufs" => opt_num(m.integrated_lufs, 2),
            "lra" => opt_num(m.lra, 2),
            "maxMomentaryLufs" => opt_num(m.max_momentary_lufs, 2),
            "maxShortTermLufs" => opt_num(m.max_short_term_lufs, 2),
            "plrDb" => opt_num(m.plr_db, 2),
            "stereo" => match &m.stereo {
                None => Value::Null,
                Some(s) => obj! {
                    "correlation" => num(s.correlation, 4),
                    "lowCorrelation" => opt_num(s.low_correlation, 4),
                    "monoLossDb" => num(s.mono_loss_db, 2),
                    "lowMonoLossDb" => opt_num(s.low_mono_loss_db, 2),
                    "lowNegativeFraction" => num(s.low_negative_fraction, 4),
                    "sideToMidDb" => num(s.side_to_mid_db, 2),
                    "identical" => s.identical,
                    "inverted" => s.inverted,
                    "maxAbsDifference" => num(s.max_abs_difference as f64, 6),
                },
            },
            "silence" => obj! {
                "leading" => num(m.silence.leading, 3),
                "trailing" => num(m.silence.trailing, 3),
                "silentFraction" => num(m.silence.silent_fraction, 4),
                "allSilent" => m.silence.all_silent,
                "allZero" => m.silence.all_zero,
                "dropoutCount" => m.silence.dropout_count,
                "dropouts" => Value::Array(m.silence.dropouts.iter().map(|d| obj! {
                    "start" => num(d.start, 3), "duration" => num(d.duration, 4),
                    "preDb" => num(d.pre_db, 1), "postDb" => num(d.post_db, 1),
                }).collect()),
                "abruptCutoff" => match m.silence.abrupt_cutoff {
                    None => Value::Null,
                    Some((s, l)) => obj! { "start" => num(s, 3), "length" => num(l, 3) },
                },
                "endingLevelDb" => num(m.silence.ending_level_db, 1),
            },
            "bandwidth" => obj! {
                "edge" => edge_json(&m.bandwidth.edge),
                "upperHz" => num(m.bandwidth.upper_hz, 0),
                "nyquist" => num(m.bandwidth.nyquist, 0),
                "ceilingConsistency" => opt_num(m.bandwidth.ceiling_consistency, 4),
                "breakthroughP99Db" => opt_num(m.bandwidth.breakthrough_p99_db, 1),
                "hfContent" => num(m.bandwidth.hf_content, 4),
                "sideEdge" => edge_json(&m.bandwidth.side_edge),
                "ultrasonicRelDb" => opt_num(m.bandwidth.ultrasonic_rel_db, 1),
            },
            "artifacts" => obj! {
                "loHz" => num(m.artifacts.lo_hz, 0), "hiHz" => num(m.artifacts.hi_hz, 0),
                "holeRatio" => opt_num(m.artifacts.hole_ratio, 5),
                "topHoleRatio" => opt_num(m.artifacts.top_hole_ratio, 5),
                "sfm" => opt_num(m.artifacts.sfm, 4),
                "frames" => m.artifacts.frames,
            },
            "segments" => Value::Array(m.segments.iter().map(|s| obj! {
                "start" => num(s.start, 2), "end" => num(s.end, 2), "cutoffHz" => opt_num(s.cutoff_hz, 0),
                "edgeDb" => num(s.edge_db, 1), "upperHz" => num(s.upper_hz, 0), "referenceDb" => num(s.reference_db, 1),
            }).collect()),
            "bandwidthChanges" => Value::Array(m.bandwidth_changes.iter().map(|c| obj! {
                "time" => num(c.time, 2), "fromHz" => num(c.from_hz, 0), "toHz" => num(c.to_hz, 0),
            }).collect()),
            "noise" => obj! {
                "quietSeconds" => num(m.noise.quiet_seconds, 2),
                "quietRmsDb" => opt_num(m.noise.quiet_rms_db, 1),
                "quietHfFlatness" => opt_num(m.noise.quiet_hf_flatness, 4),
            },
            "bands" => Value::Array(m.bands.iter().map(|b| obj! {
                "id" => b.id, "label" => b.label, "loHz" => num(b.lo_hz, 0),
                "hiHz" => num(b.hi_hz, 0), "share" => num(b.share, 5), "relDb" => num(b.rel_db, 2),
            }).collect()),
            "tempo" => match &m.tempo {
                None => Value::Null,
                Some(t) => obj! {
                    "bpm" => num(t.bpm, 3), "confidence" => num(t.confidence, 3), "variable" => t.variable,
                    "candidates" => Value::Array(t.candidates.iter().map(|c| Value::Array(vec![num(c.0, 3), num(c.1, 3)])).collect()),
                },
            },
            "key" => match &m.key {
                None => Value::Null,
                Some(k) => obj! {
                    "key" => k.key, "name" => format_key(k.key, Notation::Musical, true), "camelot" => format_key(k.key, Notation::Camelot, true),
                    "confidence" => num(k.confidence, 3), "second" => k.second, "tuningCents" => num(k.tuning_cents, 1),
                    "chroma" => nums(&k.chroma, 3), "changes" => k.changes,
                },
            },
            "analysisMs" => m.analysis_ms,
        }
    }
}

// -------------------------------------------------------------- from JSON

fn f(v: &Value, k: &str) -> Option<f64> {
    v.get(k).and_then(Value::as_f64)
}
fn fz(v: &Value, k: &str) -> f64 {
    f(v, k).unwrap_or(0.0)
}
fn u(v: &Value, k: &str) -> u64 {
    v.get(k).and_then(Value::as_u64).unwrap_or(0)
}
fn b(v: &Value, k: &str) -> bool {
    v.get(k).and_then(Value::as_bool).unwrap_or(false)
}
fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}
fn fv(v: &Value, k: &str) -> Vec<f64> {
    v.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default()
}
fn obj_at<'a>(v: &'a Value, k: &str) -> Option<&'a Value> {
    v.get(k).filter(|x| !x.is_null())
}
fn edge_from(v: Option<&Value>) -> Option<Edge> {
    let e = v?;
    Some(Edge {
        hz: fz(e, "hz"),
        drop_db: fz(e, "dropDb"),
        edge_db: fz(e, "edgeDb"),
        shelf_db: fz(e, "shelfDb"),
        floor_db: fz(e, "floorDb"),
        transition_hz: fz(e, "transitionHz"),
        floor_stdev_db: fz(e, "floorStdevDb"),
    })
}

impl Measurements {
    pub fn to_json(&self) -> Value {
        ToJson::to_json(self)
    }

    /// Parses a record produced by [`Measurements::to_json`]. Returns `None`
    /// if the schema version is missing or newer than this build understands.
    pub fn from_json(v: &Value) -> Option<Measurements> {
        let schema = u(v, "schema") as u32;
        if schema == 0 || schema > MEASUREMENT_SCHEMA {
            return None;
        }
        let empty = Value::Null;
        let sil = obj_at(v, "silence").unwrap_or(&empty);
        let bw = obj_at(v, "bandwidth").unwrap_or(&empty);
        let art = obj_at(v, "artifacts").unwrap_or(&empty);
        let noise = obj_at(v, "noise").unwrap_or(&empty);
        Some(Measurements {
            schema,
            engine_version: s(v, "engineVersion"),
            decoder: s(v, "decoder"),
            sample_kind: s(v, "sampleKind"),
            sample_rate: u(v, "sampleRate") as u32,
            channels: u(v, "channels") as u16,
            analyzed_channels: u(v, "analyzedChannels") as u16,
            duration: fz(v, "duration"),
            declared_duration: f(v, "declaredDuration"),
            decode_errors: u(v, "decodeErrors") as u32,
            decode_messages: v
                .get("decodeMessages")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
                .unwrap_or_default(),
            truncated: b(v, "truncated"),
            checksum: s(v, "checksum"),
            container_bits: v.get("containerBits").and_then(Value::as_u64).map(|x| x as u16),
            effective_bits: v.get("effectiveBits").and_then(Value::as_u64).map(|x| x as u16),
            peak_dbfs: fz(v, "peakDbfs"),
            channel_peak_dbfs: fv(v, "channelPeakDbfs"),
            true_peak_dbtp: fz(v, "truePeakDbtp"),
            true_peak_oversampling: u(v, "truePeakOversampling") as u32,
            tp_over_frames: u(v, "tpOverFrames"),
            tp_over_events: u(v, "tpOverEvents"),
            rms_dbfs: fz(v, "rmsDbfs"),
            channel_rms_dbfs: fv(v, "channelRmsDbfs"),
            crest_db: fz(v, "crestDb"),
            dc_offset: fv(v, "dcOffset"),
            decoded_overs: u(v, "decodedOvers"),
            clipping: obj_at(v, "clipping").map(|c| ClipMeasure {
                events: u(c, "events"),
                samples: u(c, "samples"),
                longest_run: u(c, "longestRun") as u32,
                events_per_minute: fz(c, "eventsPerMinute"),
                ceiling_dbfs: fz(c, "ceilingDbfs"),
                clipped_fraction: fz(c, "clippedFraction"),
            }),
            integrated_lufs: f(v, "integratedLufs"),
            lra: f(v, "lra"),
            max_momentary_lufs: f(v, "maxMomentaryLufs"),
            max_short_term_lufs: f(v, "maxShortTermLufs"),
            plr_db: f(v, "plrDb"),
            stereo: obj_at(v, "stereo").map(|st| StereoResult {
                correlation: fz(st, "correlation"),
                low_correlation: f(st, "lowCorrelation"),
                mono_loss_db: fz(st, "monoLossDb"),
                low_mono_loss_db: f(st, "lowMonoLossDb"),
                low_negative_fraction: fz(st, "lowNegativeFraction"),
                side_to_mid_db: fz(st, "sideToMidDb"),
                identical: b(st, "identical"),
                inverted: b(st, "inverted"),
                max_abs_difference: fz(st, "maxAbsDifference") as f32,
            }),
            silence: SilenceMeasure {
                leading: fz(sil, "leading"),
                trailing: fz(sil, "trailing"),
                silent_fraction: fz(sil, "silentFraction"),
                all_silent: b(sil, "allSilent"),
                all_zero: b(sil, "allZero"),
                dropout_count: u(sil, "dropoutCount"),
                dropouts: sil
                    .get("dropouts")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .map(|d| Dropout { start: fz(d, "start"), duration: fz(d, "duration"), pre_db: fz(d, "preDb"), post_db: fz(d, "postDb") })
                            .collect()
                    })
                    .unwrap_or_default(),
                abrupt_cutoff: obj_at(sil, "abruptCutoff").map(|a| (fz(a, "start"), fz(a, "length"))),
                ending_level_db: fz(sil, "endingLevelDb"),
            },
            bandwidth: BandwidthMeasure {
                edge: edge_from(obj_at(bw, "edge")),
                upper_hz: fz(bw, "upperHz"),
                nyquist: fz(bw, "nyquist"),
                ceiling_consistency: f(bw, "ceilingConsistency"),
                breakthrough_p99_db: f(bw, "breakthroughP99Db"),
                hf_content: fz(bw, "hfContent"),
                side_edge: edge_from(obj_at(bw, "sideEdge")),
                ultrasonic_rel_db: f(bw, "ultrasonicRelDb"),
            },
            artifacts: ArtifactMeasure {
                lo_hz: fz(art, "loHz"),
                hi_hz: fz(art, "hiHz"),
                hole_ratio: f(art, "holeRatio"),
                top_hole_ratio: f(art, "topHoleRatio"),
                sfm: f(art, "sfm"),
                frames: u(art, "frames"),
            },
            segments: v
                .get("segments")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .map(|x| SegmentBandwidth {
                            start: fz(x, "start"),
                            end: fz(x, "end"),
                            cutoff_hz: f(x, "cutoffHz"),
                            edge_db: fz(x, "edgeDb"),
                            upper_hz: fz(x, "upperHz"),
                            reference_db: fz(x, "referenceDb"),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            bandwidth_changes: v
                .get("bandwidthChanges")
                .and_then(Value::as_array)
                .map(|a| a.iter().map(|x| BandwidthChange { time: fz(x, "time"), from_hz: fz(x, "fromHz"), to_hz: fz(x, "toHz") }).collect())
                .unwrap_or_default(),
            noise: NoiseMeasure { quiet_seconds: fz(noise, "quietSeconds"), quiet_rms_db: f(noise, "quietRmsDb"), quiet_hf_flatness: f(noise, "quietHfFlatness") },
            bands: v
                .get("bands")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .map(|x| BandShare {
                            id: s(x, "id"),
                            label: s(x, "label"),
                            lo_hz: fz(x, "loHz"),
                            hi_hz: fz(x, "hiHz"),
                            share: fz(x, "share"),
                            rel_db: fz(x, "relDb"),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            tempo: obj_at(v, "tempo").and_then(|t| {
                Some(TempoResult {
                    bpm: f(t, "bpm")?,
                    confidence: fz(t, "confidence"),
                    variable: b(t, "variable"),
                    candidates: t
                        .get("candidates")
                        .and_then(Value::as_array)
                        .map(|a| {
                            a.iter()
                                .filter_map(|c| {
                                    let c = c.as_array()?;
                                    Some((c.first()?.as_f64()?, c.get(1)?.as_f64()?))
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                })
            }),
            key: obj_at(v, "key").and_then(|k| {
                let key = k.get("key").and_then(Value::as_u64).filter(|&x| x < 24)? as u8;
                let mut chroma = [0.0; 12];
                for (o, x) in chroma.iter_mut().zip(fv(k, "chroma")) {
                    *o = x;
                }
                Some(KeyResult {
                    key,
                    confidence: fz(k, "confidence"),
                    second: k.get("second").and_then(Value::as_u64).filter(|&x| x < 24).unwrap_or(key as u64) as u8,
                    tuning_cents: fz(k, "tuningCents"),
                    chroma,
                    changes: b(k, "changes"),
                })
            }),
            analysis_ms: u(v, "analysisMs"),
        })
    }
}
