//! Assessment: turns measurements into findings, sub-scores, the three
//! headline scores, a confidence estimate and a plain-English verdict.
//!
//! This stage is deterministic and cheap. It only reads [`FileFacts`] and
//! [`Measurements`], so stored analyses can be re-assessed when the scoring
//! model changes without decoding audio again.
//!
//! ## Model (see `docs/SCORING.md` for the full rationale)
//!
//! - Every [`Finding`] belongs to one **sub-score category** (what it says
//!   about the file) and one **root-cause family** (why it happened).
//! - Each finding carries point deductions for the three profiles
//!   (overall, DJ, listening), an [`Evidence`] level and a confidence.
//!   Inferred findings are weighted by their confidence; measured facts count
//!   in full.
//! - **Sub-scores** = 100 − deductions of the findings in that category, so a
//!   single root cause can show up in several sub-scores.
//! - **Headline scores** combine by family to avoid double counting
//!   correlated defects: within a family the largest deduction counts in
//!   full and every further deduction counts at 25 %; families add up.
//! - Assessors implement the [`Assessor`] trait. The built-in assessors are
//!   deterministic DSP rules; a trained model can be added later as another
//!   assessor without changing the scoring stage.

pub mod rules;

use crate::analysis::measure::Measurements;
use crate::meta::FileFacts;
use sonora_json::{num, obj, ToJson, Value};

/// Version of the scoring model. Stored with every result.
/// 1.1: no lossy cut-off in the anti-alias band of low sample rates, no
/// bandwidth finding for sounds too short to measure (see docs/SCORING.md).
pub const SCORE_MODEL_VERSION: &str = "sonora-dsp-1.1";

/// How a conclusion was reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Evidence {
    /// Directly measured or read from the bitstream (e.g. codec, clipped samples).
    Fact,
    /// Several independent indicators agree; alternative explanations are unlikely.
    Strong,
    /// A single indicator or a pattern with plausible innocent explanations.
    Weak,
    /// Not enough signal content or information to decide.
    Inconclusive,
}

impl Evidence {
    pub fn id(self) -> &'static str {
        match self {
            Evidence::Fact => "fact",
            Evidence::Strong => "strong",
            Evidence::Weak => "weak",
            Evidence::Inconclusive => "inconclusive",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Informational; no deduction.
    Info,
    Low,
    Moderate,
    High,
    Critical,
}

impl Severity {
    pub fn id(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Low => "low",
            Severity::Moderate => "moderate",
            Severity::High => "high",
            Severity::Critical => "critical",
        }
    }
}

/// Sub-score categories shown in the quality breakdown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    SourceIntegrity,
    CompressionArtifacts,
    ClippingDistortion,
    NoiseArtifacts,
    FrequencyIntegrity,
    /// Context that is not part of any sub-score (loudness, silence...).
    General,
}

impl Category {
    pub const SCORED: [Category; 5] = [
        Category::SourceIntegrity,
        Category::CompressionArtifacts,
        Category::ClippingDistortion,
        Category::NoiseArtifacts,
        Category::FrequencyIntegrity,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Category::SourceIntegrity => "source",
            Category::CompressionArtifacts => "artifacts",
            Category::ClippingDistortion => "clipping",
            Category::NoiseArtifacts => "noise",
            Category::FrequencyIntegrity => "frequency",
            Category::General => "general",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Category::SourceIntegrity => "Source & encoding integrity",
            Category::CompressionArtifacts => "Compression-artifact evidence",
            Category::ClippingDistortion => "Clipping & distortion",
            Category::NoiseArtifacts => "Noise & unwanted artifacts",
            Category::FrequencyIntegrity => "Frequency integrity",
            Category::General => "General",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Category::SourceIntegrity => {
                "Whether the file is what it claims to be: codec and bitrate, signs of a lossy source in a lossless container, decoding errors, truncation, padded bit depth and upsampling."
            }
            Category::CompressionArtifacts => {
                "Measurable traces of lossy coding: spectral holes from quantised-away coefficients and band-limited side (stereo) information."
            }
            Category::ClippingDistortion => {
                "Hard clipping (flat-topped waveforms), samples decoded above full scale and inter-sample (true-peak) overs. Loudness itself is never penalised."
            }
            Category::NoiseArtifacts => "Dropouts, DC offset, broadband noise in quiet passages and stereo/phase defects.",
            Category::FrequencyIntegrity => {
                "Whether the frequency range the format can carry is actually used and consistent over time: steep cutoffs, empty ultrasonic bands in hi-res files and abrupt bandwidth changes."
            }
            Category::General => "Context about the signal that does not affect scores.",
        }
    }
}

/// Root cause, used to avoid double counting correlated findings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Family {
    Lossy,
    Integrity,
    Clipping,
    Noise,
    Stereo,
    Resolution,
    Signal,
}

impl Family {
    pub fn id(self) -> &'static str {
        match self {
            Family::Lossy => "lossy",
            Family::Integrity => "integrity",
            Family::Clipping => "clipping",
            Family::Noise => "noise",
            Family::Stereo => "stereo",
            Family::Resolution => "resolution",
            Family::Signal => "signal",
        }
    }
}

/// Point deductions per scoring profile.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Penalty {
    pub overall: f64,
    pub dj: f64,
    pub listening: f64,
}

impl Penalty {
    pub const NONE: Penalty = Penalty { overall: 0.0, dj: 0.0, listening: 0.0 };

    pub fn new(overall: f64, dj: f64, listening: f64) -> Penalty {
        Penalty { overall, dj, listening }
    }

    pub fn scaled(self, k: f64) -> Penalty {
        Penalty { overall: self.overall * k, dj: self.dj * k, listening: self.listening * k }
    }

    pub fn is_zero(&self) -> bool {
        self.overall == 0.0 && self.dj == 0.0 && self.listening == 0.0
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    /// Stable machine id (e.g. "clipping", "lossy_source_suspected").
    pub id: String,
    pub category: Category,
    pub family: Family,
    pub severity: Severity,
    pub evidence: Evidence,
    /// 0–1: how likely the conclusion is correct given the evidence.
    pub confidence: f64,
    pub title: String,
    /// Short phrase for verdicts and chips (e.g. "hard clipping, 42/min").
    pub summary: String,
    /// What was detected, with the key numbers.
    pub detail: String,
    /// Why it matters.
    pub impact: String,
    /// How reliable the conclusion is and what could explain it otherwise.
    pub reliability: String,
    pub recommendation: Option<String>,
    /// Nominal deductions before confidence weighting.
    pub penalty: Penalty,
    /// Key measured values (label, value) for display.
    pub metrics: Vec<(String, String)>,
    /// Positions in seconds worth marking on the timeline.
    pub times: Vec<f64>,
}

impl Finding {
    /// Deductions after weighting inferred findings by their confidence.
    pub fn effective_penalty(&self) -> Penalty {
        match self.evidence {
            Evidence::Fact => self.penalty,
            Evidence::Inconclusive => Penalty::NONE,
            _ => self.penalty.scaled(self.confidence.clamp(0.0, 1.0)),
        }
    }
}

/// Configurable weights. Defaults reproduce [`SCORE_MODEL_VERSION`].
#[derive(Clone, Debug, PartialEq)]
pub struct ScoringConfig {
    /// Weight of additional findings within the same root-cause family.
    pub correlated_weight: f64,
    /// Global multiplier on all deductions per profile.
    pub overall_scale: f64,
    pub dj_scale: f64,
    pub listening_scale: f64,
}

impl Default for ScoringConfig {
    fn default() -> Self {
        ScoringConfig { correlated_weight: 0.25, overall_scale: 1.0, dj_scale: 1.0, listening_scale: 1.0 }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SubScore {
    pub category: Category,
    pub score: f64,
    /// Ids of the findings that lowered it.
    pub findings: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Scores {
    pub overall: f64,
    pub dj: f64,
    pub listening: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FamilyDeduction {
    pub family: Family,
    pub overall: f64,
    pub dj: f64,
    pub listening: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Assessment {
    pub model_version: String,
    pub scores: Scores,
    pub subscores: Vec<SubScore>,
    pub deductions: Vec<FamilyDeduction>,
    /// 0–1.
    pub confidence: f64,
    pub confidence_label: String,
    pub confidence_reasons: Vec<String>,
    pub verdict: String,
    /// Short machine flags for filtering (e.g. "clipping", "lossy_source").
    pub flags: Vec<String>,
    pub findings: Vec<Finding>,
}

/// Context handed to assessors.
pub struct AssessContext<'a> {
    pub facts: &'a FileFacts,
    pub m: &'a Measurements,
}

/// A source of findings. The built-in assessors are deterministic DSP rules;
/// future machine-learning models can implement this trait and be added to
/// [`assessors`] without changing how findings are scored.
pub trait Assessor: Send + Sync {
    fn id(&self) -> &'static str;
    fn version(&self) -> &'static str;
    fn assess(&self, ctx: &AssessContext) -> Vec<Finding>;
}

/// The assessors used by [`assess`].
pub fn assessors() -> Vec<Box<dyn Assessor>> {
    rules::builtin()
}

fn combine(findings: &[Finding], cfg: &ScoringConfig) -> (Scores, Vec<FamilyDeduction>) {
    let mut families: Vec<Family> = Vec::new();
    for f in findings {
        if !families.contains(&f.family) {
            families.push(f.family);
        }
    }
    let mut deductions = Vec::new();
    let mut total = Penalty::NONE;
    for fam in families {
        let ps: Vec<Penalty> = findings.iter().filter(|f| f.family == fam).map(|f| f.effective_penalty()).collect();
        let agg = |sel: &dyn Fn(&Penalty) -> f64| -> f64 {
            let mut v: Vec<f64> = ps.iter().map(sel).filter(|x| *x > 0.0).collect();
            v.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
            match v.split_first() {
                None => 0.0,
                Some((first, rest)) => first + cfg.correlated_weight * rest.iter().sum::<f64>(),
            }
        };
        let d = FamilyDeduction { family: fam, overall: agg(&|p| p.overall), dj: agg(&|p| p.dj), listening: agg(&|p| p.listening) };
        if d.overall > 0.0 || d.dj > 0.0 || d.listening > 0.0 {
            total.overall += d.overall;
            total.dj += d.dj;
            total.listening += d.listening;
            deductions.push(d);
        }
    }
    let clamp = |x: f64| x.clamp(0.0, 100.0).round();
    (
        Scores {
            overall: clamp(100.0 - total.overall * cfg.overall_scale),
            dj: clamp(100.0 - total.dj * cfg.dj_scale),
            listening: clamp(100.0 - total.listening * cfg.listening_scale),
        },
        deductions,
    )
}

fn subscores(findings: &[Finding]) -> Vec<SubScore> {
    Category::SCORED
        .iter()
        .map(|&cat| {
            let members: Vec<&Finding> = findings.iter().filter(|f| f.category == cat).collect();
            let mut v: Vec<f64> = members.iter().map(|f| f.effective_penalty().overall).filter(|x| *x > 0.0).collect();
            v.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
            // Within a category the same diminishing rule applies.
            let d = match v.split_first() {
                None => 0.0,
                Some((first, rest)) => first + 0.25 * rest.iter().sum::<f64>(),
            };
            SubScore {
                category: cat,
                score: (100.0 - d).clamp(0.0, 100.0).round(),
                findings: members.iter().filter(|f| !f.effective_penalty().is_zero()).map(|f| f.id.clone()).collect(),
            }
        })
        .collect()
}

/// Runs all assessors and scores the result.
pub fn assess(facts: &FileFacts, m: &Measurements, cfg: &ScoringConfig) -> Assessment {
    let ctx = AssessContext { facts, m };
    let mut findings: Vec<Finding> = assessors().iter().flat_map(|a| a.assess(&ctx)).collect();
    // Most important first: severity, then evidence strength, then deduction.
    findings.sort_by(|a, b| {
        b.severity
            .cmp(&a.severity)
            .then(a.evidence.cmp(&b.evidence))
            .then(b.effective_penalty().overall.partial_cmp(&a.effective_penalty().overall).unwrap_or(std::cmp::Ordering::Equal))
    });
    let (scores, deductions) = combine(&findings, cfg);
    let subscores = subscores(&findings);
    let (confidence, reasons) = rules::confidence(&ctx, &findings);
    let label = if confidence >= 0.8 {
        "High"
    } else if confidence >= 0.6 {
        "Medium"
    } else {
        "Low"
    };
    let verdict = rules::verdict(&ctx, &findings, &scores);
    let flags = rules::flags(&findings);
    Assessment {
        model_version: SCORE_MODEL_VERSION.to_string(),
        scores,
        subscores,
        deductions,
        confidence,
        confidence_label: label.to_string(),
        confidence_reasons: reasons,
        verdict,
        flags,
        findings,
    }
}

impl ToJson for Finding {
    fn to_json(&self) -> Value {
        let eff = self.effective_penalty();
        obj! {
            "id" => self.id,
            "category" => self.category.id(),
            "family" => self.family.id(),
            "severity" => self.severity.id(),
            "evidence" => self.evidence.id(),
            "confidence" => num(self.confidence, 2),
            "title" => self.title,
            "summary" => self.summary,
            "detail" => self.detail,
            "impact" => self.impact,
            "reliability" => self.reliability,
            "recommendation" => self.recommendation,
            "penalty" => obj! {
                "overall" => num(eff.overall, 1), "dj" => num(eff.dj, 1), "listening" => num(eff.listening, 1),
                "nominalOverall" => num(self.penalty.overall, 1),
            },
            "metrics" => Value::Array(self.metrics.iter().map(|(k, v)| obj! { "label" => k, "value" => v }).collect()),
            "times" => Value::Array(self.times.iter().map(|&t| num(t, 2)).collect()),
        }
    }
}

impl ToJson for Assessment {
    fn to_json(&self) -> Value {
        obj! {
            "modelVersion" => self.model_version,
            "scores" => obj! { "overall" => self.scores.overall, "dj" => self.scores.dj, "listening" => self.scores.listening },
            "subscores" => Value::Array(self.subscores.iter().map(|s| obj! {
                "id" => s.category.id(), "label" => s.category.label(), "description" => s.category.description(),
                "score" => s.score, "findings" => s.findings,
            }).collect()),
            "deductions" => Value::Array(self.deductions.iter().map(|d| obj! {
                "family" => d.family.id(), "overall" => num(d.overall, 1), "dj" => num(d.dj, 1), "listening" => num(d.listening, 1),
            }).collect()),
            "confidence" => num(self.confidence, 2),
            "confidenceLabel" => self.confidence_label,
            "confidenceReasons" => self.confidence_reasons,
            "verdict" => self.verdict,
            "flags" => self.flags,
            "findings" => self.findings,
        }
    }
}
