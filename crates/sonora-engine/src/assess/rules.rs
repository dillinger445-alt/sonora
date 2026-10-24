//! Built-in deterministic assessment rules (model `sonora-dsp-1.1`).
//!
//! Thresholds were calibrated against real encodes (LAME MP3 128/320/V0,
//! FFmpeg AAC 128/256, libopus 160, libvorbis q6), lossless transcodes of
//! those, and deliberately ambiguous lossless material (a steep 16 kHz
//! low-pass, a dark master). `docs/SCORING.md` documents every rule.

use super::{AssessContext, Assessor, Category, Evidence, Family, Finding, Penalty, Scores, Severity};
use crate::analysis::bandwidth::{Edge, STEEP_DROP_DB};
use crate::meta::FileFacts;
use crate::util::fmt_time;

// ------------------------------------------------------------------ helpers

fn khz(hz: f64) -> String {
    format!("{:.1} kHz", hz / 1000.0)
}

fn pct(x: f64) -> String {
    if x < 0.001 {
        format!("{:.3} %", x * 100.0)
    } else if x < 0.1 {
        format!("{:.2} %", x * 100.0)
    } else {
        format!("{:.1} %", x * 100.0)
    }
}

fn plural(n: u64, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

fn times_list(times: &[f64], max: usize) -> String {
    let mut parts: Vec<String> = times.iter().take(max).map(|&t| fmt_time(t)).collect();
    if times.len() > max {
        parts.push(format!("+{} more", times.len() - max));
    }
    parts.join(", ")
}

fn new(id: &str, category: Category, family: Family) -> Finding {
    Finding {
        id: id.to_string(),
        category,
        family,
        severity: Severity::Info,
        evidence: Evidence::Fact,
        confidence: 1.0,
        title: String::new(),
        summary: String::new(),
        detail: String::new(),
        impact: String::new(),
        reliability: String::new(),
        recommendation: None,
        penalty: Penalty::NONE,
        metrics: Vec::new(),
        times: Vec::new(),
    }
}

impl Finding {
    fn sev(mut self, s: Severity) -> Self {
        self.severity = s;
        self
    }
    fn ev(mut self, e: Evidence, confidence: f64) -> Self {
        self.evidence = e;
        self.confidence = confidence.clamp(0.0, 1.0);
        self
    }
    fn text(mut self, title: impl Into<String>, summary: impl Into<String>, detail: impl Into<String>) -> Self {
        self.title = title.into();
        self.summary = summary.into();
        self.detail = detail.into();
        self
    }
    fn why(mut self, impact: impl Into<String>, reliability: impl Into<String>) -> Self {
        self.impact = impact.into();
        self.reliability = reliability.into();
        self
    }
    fn rec(mut self, r: impl Into<String>) -> Self {
        self.recommendation = Some(r.into());
        self
    }
    fn pen(mut self, overall: f64, dj: f64, listening: f64) -> Self {
        self.penalty = Penalty::new(overall, dj, listening);
        self
    }
    fn metric(mut self, label: &str, value: impl Into<String>) -> Self {
        self.metrics.push((label.to_string(), value.into()));
        self
    }
    fn at(mut self, times: Vec<f64>) -> Self {
        self.times = times;
        self
    }
}

/// Deduction (overall profile) for lossy audio of a given MP3-equivalent
/// bitrate. Interpolated on a log2 bitrate scale.
pub fn lossy_penalty(eq_kbps: f64) -> f64 {
    const PTS: [(f64, f64); 9] = [(64.0, 50.0), (96.0, 38.0), (128.0, 28.0), (160.0, 20.0), (192.0, 15.0), (224.0, 12.0), (256.0, 10.0), (320.0, 8.0), (448.0, 6.0)];
    let x = eq_kbps.max(1.0).log2();
    if x <= PTS[0].0.log2() {
        return PTS[0].1;
    }
    for w in PTS.windows(2) {
        let (x0, y0) = (w[0].0.log2(), w[0].1);
        let (x1, y1) = (w[1].0.log2(), w[1].1);
        if x <= x1 {
            return y0 + (y1 - y0) * (x - x0) / (x1 - x0);
        }
    }
    PTS[PTS.len() - 1].1
}

/// Relative coding efficiency versus MP3 at the same bitrate (approximate,
/// from published listening-test consensus).
fn codec_efficiency(codec: &str, profile: Option<&str>) -> f64 {
    match codec {
        "mp3" => 1.0,
        "mp2" | "mp1" => 0.75,
        "aac" => match profile {
            Some(p) if p.contains("HE") => 1.4,
            _ => 1.3,
        },
        "vorbis" => 1.25,
        "opus" => 1.5,
        "ac3" => 0.8,
        "eac3" => 1.0,
        _ => 1.0,
    }
}

/// Source class implied by a lossy-style cutoff, from the LAME low-pass
/// table (the measured half-way point sits ~0.2 kHz below the setting).
fn implied_from_cutoff(hz: f64) -> (f64, &'static str) {
    if hz < 14_000.0 {
        (80.0, "a low-bitrate lossy source (≤ 96 kbps MP3 class)")
    } else if hz < 15_800.0 {
        (104.0, "a ~96–112 kbps lossy source")
    } else if hz < 17_200.0 {
        (128.0, "a ~128 kbps lossy source")
    } else if hz < 18_200.0 {
        (160.0, "a ~160 kbps lossy source")
    } else if hz < 19_100.0 {
        (192.0, "a ~192 kbps lossy source")
    } else if hz < 19_600.0 {
        (224.0, "a ~224 kbps lossy source")
    } else if hz < 20_100.0 {
        (256.0, "a ~256 kbps lossy source")
    } else {
        (320.0, "a high-bitrate lossy source")
    }
}

/// Bandwidth an encoder normally keeps at a given bitrate (conservative).
fn expected_min_bandwidth(codec: &str, kbps: f64) -> Option<f64> {
    match codec {
        "mp3" if kbps >= 250.0 => Some(19_000.0),
        "mp3" if kbps >= 185.0 => Some(17_800.0),
        "mp3" if kbps >= 155.0 => Some(16_800.0),
        "aac" if kbps >= 240.0 => Some(18_500.0),
        "aac" if kbps >= 180.0 => Some(17_000.0),
        "opus" if kbps >= 96.0 => Some(19_500.0),
        "vorbis" if kbps >= 190.0 => Some(18_500.0),
        _ => None,
    }
}

/// A steep edge in the spectrum, as an encoder's low-pass leaves it. An edge
/// within 1.5 kHz of the top frequency the sample rate allows is the
/// anti-alias filter every converter and resampler has, not an encoder's: a
/// lossless 32 kHz file falls steeply just below 16 kHz (B34). At 44.1 and
/// 48 kHz such edges were above every cut-off the rules look at anyway.
fn steep_edge<'a>(ctx: &AssessContext<'a>) -> Option<&'a Edge> {
    let nyquist = ctx.m.sample_rate as f64 / 2.0;
    ctx.m.bandwidth.edge.as_ref().filter(|e| e.is_steep() && nyquist - e.hz > 1500.0)
}

fn is_lossless(f: &FileFacts) -> bool {
    f.lossless == Some(true)
}

fn is_lossy(f: &FileFacts) -> bool {
    f.lossless == Some(false)
}

fn has_signal(ctx: &AssessContext) -> bool {
    !ctx.m.silence.all_silent
}

// --------------------------------------------------------------- integrity

struct IntegrityRules;

impl Assessor for IntegrityRules {
    fn id(&self) -> &'static str {
        "integrity"
    }
    fn version(&self) -> &'static str {
        "1.0"
    }
    fn assess(&self, ctx: &AssessContext) -> Vec<Finding> {
        let m = ctx.m;
        let mut out = Vec::new();

        if m.silence.all_zero {
            out.push(
                new("silent_file", Category::SourceIntegrity, Family::Signal)
                    .sev(Severity::Critical)
                    .text("No audio signal", "the file is digital silence", format!("The file decodes to {} of digital silence.", fmt_time(m.duration)))
                    .why("There is nothing to play. Files like this usually come from failed exports or downloads.", "Measured: every decoded sample is exactly zero.")
                    .rec("Re-export or re-download the track.")
                    .pen(100.0, 100.0, 100.0),
            );
            return out;
        }
        if m.silence.all_silent {
            out.push(
                new("near_silent", Category::SourceIntegrity, Family::Signal)
                    .sev(Severity::High)
                    .text("Audio never rises above -60 dBFS", "barely any signal", format!("The highest peak is {:.1} dBFS over {}.", m.peak_dbfs, fmt_time(m.duration)))
                    .why("The file is practically silent; it is unlikely to be the intended audio.", "Measured from the decoded samples.")
                    .pen(60.0, 80.0, 70.0),
            );
        }

        if m.truncated {
            let detail = match m.declared_duration {
                Some(d) if d > m.duration => format!("Decoding produced {} of audio, but the file declares {}. The last {} are missing.", fmt_time(m.duration), fmt_time(d), fmt_time(d - m.duration)),
                _ => "The file ends before the end of the audio data its header declares.".to_string(),
            };
            out.push(
                new("truncated", Category::SourceIntegrity, Family::Integrity)
                    .sev(Severity::Critical)
                    .text("File is truncated", "the file is truncated", detail)
                    .why("Part of the track is missing; playback stops early or ends with a glitch.", "Fact: the decoded length is shorter than the length stored in the file header.")
                    .rec("Re-download or restore the file from its original source.")
                    .pen(45.0, 60.0, 50.0)
                    .at(vec![m.duration])
                    .metric("Decoded", fmt_time(m.duration))
                    .metric("Declared", m.declared_duration.map(fmt_time).unwrap_or_else(|| "–".into())),
            );
        }

        if m.decode_errors > 0 {
            let n = m.decode_errors as u64;
            let first = m.decode_messages.first().cloned().unwrap_or_default();
            out.push(
                new("decode_errors", Category::SourceIntegrity, Family::Integrity)
                    .sev(if n > 5 { Severity::High } else { Severity::Moderate })
                    .text(
                        "Decoding errors",
                        format!("{} while decoding", plural(n, "error", "errors")),
                        format!("The decoder reported {}.{}", plural(n, "error", "errors"), if first.is_empty() { String::new() } else { format!(" First message: \u{201c}{first}\u{201d}") }),
                    )
                    .why("Damaged frames are usually heard as clicks, glitches or short gaps.", "Fact: reported by the decoder while reading the bitstream.")
                    .rec("Compare with another copy of the track; re-download if possible.")
                    .pen(10.0 + 5.0 * (n.min(6) as f64), 12.0 + 6.0 * (n.min(6) as f64), 10.0 + 5.0 * (n.min(6) as f64)),
            );
        }

        match m.checksum.as_str() {
            "mismatch" if !m.truncated => out.push(
                new("checksum_mismatch", Category::SourceIntegrity, Family::Integrity)
                    .sev(Severity::High)
                    .text("FLAC checksum mismatch", "FLAC checksum mismatch", "The MD5 checksum stored in the FLAC file does not match the decoded audio.")
                    .why("The audio data has been altered or damaged since it was encoded.", "Fact: the encoder's checksum of the original audio was compared with the decoded audio.")
                    .rec("Re-download or restore the file from its original source.")
                    .pen(30.0, 40.0, 30.0),
            ),
            "verified" => out.push(
                new("checksum_verified", Category::SourceIntegrity, Family::Integrity)
                    .text("FLAC integrity verified", "FLAC checksum verified", "The decoded audio matches the MD5 checksum stored by the encoder.")
                    .why("The audio data is bit-for-bit what the encoder received.", "Fact: checksum comparison.")
                    .pen(0.0, 0.0, 0.0),
            ),
            _ => {}
        }

        if m.silence.dropout_count > 0 {
            let n = m.silence.dropout_count;
            let longest = m.silence.dropouts.iter().map(|d| d.duration).fold(0.0, f64::max);
            let times: Vec<f64> = m.silence.dropouts.iter().map(|d| d.start).collect();
            let base = (15.0 + 7.0 * (n.saturating_sub(1) as f64)).min(40.0);
            out.push(
                new("dropouts", Category::NoiseArtifacts, Family::Integrity)
                    .sev(Severity::High)
                    .text(
                        "Dropouts inside the music",
                        format!("{} of digital silence", plural(n, "dropout", "dropouts")),
                        format!(
                            "{} where the audio drops to exact digital silence (longest {:.0} ms) at {}.",
                            plural(n, "gap", "gaps"),
                            longest * 1000.0,
                            times_list(&times, 5)
                        ),
                    )
                    .why("Music almost never jumps from a loud passage to exact digital zero and back. Gaps like this are heard as hard cuts and usually come from damaged rips, transfers or downloads.", "Fact: measured as runs of exact zero on every channel with loud audio on both sides.")
                    .rec("Replace the file with an undamaged copy before playing it out.")
                    .pen(base, base * 1.4, base * 1.1)
                    .at(times),
            );
        }

        if let Some((start, len)) = m.silence.abrupt_cutoff {
            out.push(
                new("abrupt_ending", Category::SourceIntegrity, Family::Integrity)
                    .sev(Severity::High)
                    .ev(Evidence::Strong, 0.85)
                    .text(
                        "Audio stops abruptly",
                        "audio stops abruptly (possibly incomplete download)",
                        format!("Loud audio stops abruptly at {} and the remaining {} are digital silence.", fmt_time(start), fmt_time(len)),
                    )
                    .why("This is the typical signature of an incomplete download whose missing part was filled with zeros.", "Strong indicator: an intentional ending would normally decay or fade; a hard stop into a long stretch of exact zeros is rarely deliberate.")
                    .rec("Re-download the file and compare its length with the original release.")
                    .pen(35.0, 50.0, 35.0)
                    .at(vec![start]),
            );
        }

        if let Some(msg) = &ctx.facts.extension_mismatch {
            out.push(
                new("extension_mismatch", Category::SourceIntegrity, Family::Integrity)
                    .sev(Severity::Low)
                    .text("File extension does not match content", "extension does not match content", msg.clone())
                    .why("Misnamed files mislead people and software about what they contain, e.g. a lossy file posing as lossless.", "Fact: the format was identified from the file content, not its name.")
                    .rec("Rename the file to match its actual format.")
                    .pen(3.0, 2.0, 1.0),
            );
        }
        out
    }
}

// ------------------------------------------------------------------ source

struct SourceRules;

impl SourceRules {
    fn lossy_file(&self, ctx: &AssessContext, out: &mut Vec<Finding>) {
        let f = ctx.facts;
        let m = ctx.m;
        let codec = f.codec_id.as_str();
        let channels = f.channels.max(1) as f64;
        let kbps = f.bitrate_kbps.filter(|k| *k > 0.0);
        let eq = kbps.map(|k| k * codec_efficiency(codec, f.codec_profile.as_deref()) * (2.0 / channels.min(2.0)));
        let base = eq.map(lossy_penalty).unwrap_or(15.0);
        let quality = match eq {
            Some(e) if e >= 250.0 => "usually transparent for most listeners",
            Some(e) if e >= 180.0 => "usually close to transparent, though artifacts can appear on demanding material",
            Some(e) if e >= 140.0 => "often audibly degraded on good headphones and large systems",
            Some(_) => "audibly degraded on good systems",
            None => "of unknown quality (the bitrate could not be determined)",
        };
        let rate = kbps.map(|k| format!(" at {:.0} kbps", k)).unwrap_or_default();
        let mode = f.bitrate_mode.as_ref().map(|m| format!(" {m}")).unwrap_or_default();
        let sev = match eq {
            Some(e) if e >= 250.0 => Severity::Low,
            Some(e) if e >= 160.0 => Severity::Moderate,
            Some(_) => Severity::High,
            None => Severity::Moderate,
        };
        out.push(
            new("lossy_codec", Category::SourceIntegrity, Family::Lossy)
                .sev(sev)
                .text(
                    format!("Lossy encoding: {}{}{}", f.codec_name, rate, mode),
                    format!("lossy {}{}", f.codec_name, kbps.map(|k| format!(" {:.0} kbps", k)).unwrap_or_default()),
                    format!("Encoded with {}{}{}. Lossy codecs permanently discard detail the encoder judges inaudible; at this bitrate the result is {}.", f.codec_name, rate, mode, quality),
                )
                .why(
                    "Lossy files cannot be restored to the original quality, and artifacts become more noticeable at high playback levels and when the file is processed (pitch, EQ, time-stretch).",
                    "Fact: the codec and bitrate are read from the file itself. An MP3 is not automatically poor: the bitrate and the measurements below matter more than the format.",
                )
                .pen(base, base * 1.25, base * 1.1)
                .metric("Codec", f.codec_name.clone())
                .metric("Bitrate", kbps.map(|k| format!("{:.0} kbps", k)).unwrap_or_else(|| "unknown".into())),
        );

        // Bandwidth lower than the bitrate would normally keep: a re-encode
        // from a lower-quality source ("fake 320").
        if let Some(e) = steep_edge(ctx) {
            let lame = f.encoder_lowpass_hz.map(|v| v as f64).filter(|v| *v > 5000.0);
            let expected = lame.map(|l| l - 400.0).or_else(|| kbps.and_then(|k| expected_min_bandwidth(codec, k)));
            if let Some(exp) = expected {
                if e.hz < exp - 1000.0 && m.bandwidth.hf_content >= 0.3 {
                    let (implied_kbps, label) = implied_from_cutoff(e.hz);
                    let (evidence, conf) = if lame.is_some() || e.hz < 17_500.0 { (Evidence::Strong, 0.85) } else { (Evidence::Weak, 0.5) };
                    let p = lossy_penalty(implied_kbps);
                    let basis = match lame {
                        Some(l) => format!("the encoder's own setting stored in the LAME header was {}", khz(l)),
                        None => format!("encoders normally keep content up to about {} at this bitrate", khz(exp)),
                    };
                    out.push(
                        new("bandwidth_below_bitrate", Category::SourceIntegrity, Family::Lossy)
                            .sev(Severity::High)
                            .ev(evidence, conf)
                            .text(
                                "Bandwidth suggests a lower-quality source",
                                format!("likely re-encoded from {label}"),
                                format!(
                                    "This {}{} file has no content above {}, but {}. The pattern is typical of a file re-encoded from {} (for example a 128 kbps file saved as 320 kbps).",
                                    // "320 kbps MP3", not "at 320 kbps" (B77).
                                    kbps.map(|k| format!("{k:.0} kbps ")).unwrap_or_default(),
                                    f.codec_name,
                                    khz(e.hz),
                                    basis,
                                    label
                                ),
                            )
                            .why(
                                "The audio quality is limited by the earlier encode; the higher bitrate only makes the file larger.",
                                if lame.is_some() {
                                    "Strong indicator: the encoder recorded a higher low-pass than the audio actually contains, so the missing band was already absent from its input."
                                } else {
                                    "The cutoff is a clue, not proof: unusual encoder settings or a band-limited master can produce the same picture."
                                },
                            )
                            .rec("Look for a better source (lossless or a genuine high-bitrate file).")
                            .pen(p, p * 1.25, p * 1.1)
                            .metric("Cutoff", khz(e.hz))
                            .metric("Expected", khz(exp)),
                    );
                }
            }
        }
    }

    fn lossless_container(&self, ctx: &AssessContext, out: &mut Vec<Finding>) {
        let m = ctx.m;
        let bw = &m.bandwidth;
        // Too short for a single spectrum frame: nothing to say about the
        // source's bandwidth (B78), rather than "fades out around 0.0 kHz".
        if bw.upper_hz <= 0.0 {
            return;
        }
        let holes = m.artifacts.top_hole_ratio.unwrap_or(0.0);
        let Some(e) = steep_edge(ctx).filter(|e| e.hz < 20_500.0) else {
            // No encoder-style cutoff.
            if bw.upper_hz < 15_000.0 || (bw.upper_hz < 18_000.0 && m.bandwidth.edge.is_none()) {
                out.push(
                    new("source_inconclusive", Category::SourceIntegrity, Family::Lossy)
                        .ev(Evidence::Inconclusive, 0.5)
                        .text(
                            "Original source quality cannot be determined",
                            "source quality inconclusive",
                            format!("Content fades out gradually around {} without an encoder-style cutoff. There is too little high-frequency content to tell a lossless source from a lossy one.", khz(bw.upper_hz)),
                        )
                        .why("Dark or warm productions are often intentional; they simply carry less evidence about their origin.", "Inconclusive: lossy-source detection relies on high-frequency content, which this track has little of.")
                        .metric("Upper bandwidth", khz(bw.upper_hz)),
                );
            } else {
                out.push(
                    new("no_lossy_indicators", Category::SourceIntegrity, Family::Lossy)
                        .ev(Evidence::Inconclusive, 0.6)
                        .text(
                            "No signs of a lossy source",
                            "full bandwidth",
                            format!("Content extends to {} without an encoder-style cutoff, and no excess spectral holes were measured.", khz(bw.upper_hz)),
                        )
                        .why(
                            "A lossless file made from a lossy source usually shows a steep cutoff below 20 kHz.",
                            "This lowers the likelihood of a lossy source but cannot rule it out: high-bitrate encodes (e.g. AAC 256 kbps or MP3 V0) can keep the full bandwidth and leave no measurable trace.",
                        )
                        .metric("Upper bandwidth", khz(bw.upper_hz)),
                );
            }
            return;
        };

        // Points from independent indicators.
        let mut pts: f64 = if e.hz < 16_500.0 {
            3.0
        } else if e.hz < 17_500.0 {
            2.6
        } else if e.hz < 19_000.0 {
            2.2
        } else if e.hz < 19_800.0 {
            1.4
        } else {
            0.6
        };
        let mut notes: Vec<String> = vec![format!("a steep cutoff at {} (level drops {:.0} dB within {:.1} kHz)", khz(e.hz), e.drop_db, e.transition_hz / 1000.0)];
        match bw.breakthrough_p99_db {
            Some(p) if p <= -22.0 => {
                pts += 1.0;
                notes.push("nothing ever crosses that ceiling, even on the loudest high-frequency hits".into());
            }
            Some(p) if p <= -15.0 => pts += 0.4,
            Some(_) => {
                pts -= 0.5;
                notes.push("energy occasionally leaks above the cutoff, which is more typical of an analogue-style filter".into());
            }
            None => {}
        }
        if bw.ceiling_consistency.unwrap_or(0.0) >= 0.9 {
            pts += 0.3;
        }
        if holes >= 0.025 {
            pts += 1.2;
            notes.push(format!("{} of bins below the cutoff are spectral holes (natural content: about 0.7 %)", pct(holes)));
        } else if holes >= 0.018 {
            pts += 0.6;
            notes.push(format!("a raised share of spectral holes ({})", pct(holes)));
        }
        if let Some(se) = bw.side_edge.as_ref().filter(|s| s.is_steep() && s.hz < e.hz - 1500.0) {
            pts += 0.3;
            notes.push(format!("the stereo (side) information stops even earlier, at {}", khz(se.hz)));
        }
        let thin_evidence = bw.hf_content < 0.15;
        let (mut evidence, mut conf) = if pts >= 3.5 {
            (Evidence::Strong, (0.75 + 0.05 * (pts - 3.5)).min(0.95))
        } else if pts >= 2.0 {
            (Evidence::Weak, (0.4 + 0.1 * (pts - 2.0)).min(0.6))
        } else {
            (Evidence::Inconclusive, 0.4)
        };
        if thin_evidence {
            (evidence, conf) = match evidence {
                Evidence::Strong => (Evidence::Weak, 0.5),
                _ => (Evidence::Inconclusive, 0.3),
            };
            notes.push("but only a small part of the track carries content near the cutoff".into());
        }
        let (implied_kbps, label) = implied_from_cutoff(e.hz);
        let explanation = notes.join("; ");
        match evidence {
            Evidence::Strong | Evidence::Weak => {
                let p = lossy_penalty(implied_kbps) + 4.0;
                let strong = evidence == Evidence::Strong;
                out.push(
                    new("lossy_source_suspected", Category::SourceIntegrity, Family::Lossy)
                        .sev(if strong { Severity::High } else { Severity::Moderate })
                        .ev(evidence, conf)
                        .text(
                            if strong { "Likely made from a lossy source" } else { "Possible lossy source" },
                            if strong { format!("likely transcoded from {label}") } else { "possible lossy source".to_string() },
                            format!("This {} file shows {}. This is consistent with {}.", ctx.facts.codec_name, explanation, label),
                        )
                        .why(
                            "A lossless container cannot restore what a lossy encode removed: the file sounds like its lossy source while taking up lossless space, and is often sold or shared as higher quality than it is.",
                            if strong {
                                "Strong indicator: several independent clues agree. A cutoff is still not proof of provenance; a deliberate brick-wall filter in mastering can look similar."
                            } else {
                                "Weak indicator: the cutoff alone could also come from deliberate filtering or an unusual resampler. Listen and compare with another source."
                            },
                        )
                        .rec(if strong { "Treat it as a lossy file; look for a genuine lossless copy." } else { "Manual review recommended: compare with another copy or inspect the spectrogram." })
                        .pen(p, p * 1.25, p * 1.1)
                        .metric("Cutoff", khz(e.hz))
                        .metric("Drop", format!("{:.0} dB", e.drop_db))
                        .metric("Spectral holes", pct(holes)),
                );
            }
            _ => out.push(
                new("source_inconclusive", Category::SourceIntegrity, Family::Lossy)
                    .ev(Evidence::Inconclusive, conf)
                    .text(
                        "Source quality inconclusive",
                        "source quality inconclusive",
                        format!("This file shows {}. That is consistent with a high-bitrate lossy source, but also with an ordinary steep anti-aliasing or resampling filter.", explanation),
                    )
                    .why("High-bitrate lossy sources are hard to tell apart from lossless ones; the difference is rarely audible.", "Inconclusive: the evidence fits more than one explanation.")
                    .metric("Cutoff", khz(e.hz)),
            ),
        }
    }

    fn frequency(&self, ctx: &AssessContext, out: &mut Vec<Finding>) {
        let m = ctx.m;
        let bw = &m.bandwidth;
        let fs = m.sample_rate as f64;
        // Steep cutoff well below what the sample rate allows (measured fact).
        if let Some(e) = steep_edge(ctx).filter(|e| e.hz < 19_500.0) {
            let p = ((19_500.0 - e.hz) / 1000.0 * 5.0).clamp(2.0, 25.0);
            out.push(
                new("steep_cutoff", Category::FrequencyIntegrity, Family::Lossy)
                    .sev(if e.hz < 17_000.0 { Severity::Moderate } else { Severity::Low })
                    .text(
                        "Frequency bandwidth is limited",
                        format!("no content above {}", khz(e.hz)),
                        format!(
                            "The spectrum falls by {:.0} dB within {:.1} kHz at {} and stays at the floor above it. A {:.1} kHz file could carry content up to {}.",
                            e.drop_db,
                            e.transition_hz / 1000.0,
                            khz(e.hz),
                            fs / 1000.0,
                            khz(fs / 2.0)
                        ),
                    )
                    .why("The top octave (cymbal shimmer, air, transient detail) is missing. On big systems and good headphones this sounds duller and less open.", "Fact: measured from the long-term spectrum. The cause (lossy encoding or deliberate filtering) is assessed separately.")
                    .pen(p, p, p * 1.1)
                    .metric("Cutoff", khz(e.hz))
                    .metric("Drop", format!("{:.0} dB", e.drop_db))
                    .metric("Transition", format!("{:.0} Hz", e.transition_hz)),
            );
        }

        // Hi-res container with nothing above ~22 kHz.
        if fs >= 88_200.0 {
            if let Some(rel) = bw.ultrasonic_rel_db {
                let cut = bw.edge.as_ref().filter(|e| e.drop_db >= STEEP_DROP_DB && e.hz < 26_000.0).map(|e| e.hz).unwrap_or(bw.upper_hz);
                if rel <= -85.0 && cut <= 26_000.0 {
                    out.push(
                        new("upsampled", Category::SourceIntegrity, Family::Resolution)
                            .sev(Severity::Low)
                            .ev(Evidence::Strong, 0.85)
                            .text(
                                "Likely upsampled from 44.1/48 kHz",
                                "likely upsampled (not true hi-res)",
                                format!(
                                    "Content stops at {} and the band above 26 kHz is essentially empty ({:.0} dB relative to 2–20 kHz). This is consistent with a 44.1 or 48 kHz source converted to {:.1} kHz.",
                                    khz(cut),
                                    rel,
                                    fs / 1000.0
                                ),
                            )
                            .why("Upsampling adds no information. The audio is as good as its 44.1/48 kHz source, but the file is larger and may be sold as hi-res.", "Strong indicator: genuine high-resolution recordings almost always carry some ultrasonic content or noise.")
                            .pen(6.0, 0.0, 0.0)
                            .metric("Content up to", khz(cut))
                            .metric("Above 26 kHz", format!("{:.0} dB", rel)),
                    );
                }
            }
        }

        // Abrupt bandwidth changes over time.
        if let Some(first) = m.bandwidth_changes.first() {
            let low = first.from_hz.min(first.to_hz);
            let affected = m.segments.iter().filter(|s| s.cutoff_hz.is_some_and(|c| c <= low + 500.0)).count();
            let total = m.segments.len().max(1);
            let share = affected as f64 / total as f64;
            let strong = affected >= 2;
            let (implied_kbps, label) = implied_from_cutoff(low);
            let p = lossy_penalty(implied_kbps) * share.clamp(0.2, 1.0) + 6.0;
            let times: Vec<f64> = m.bandwidth_changes.iter().map(|c| c.time).collect();
            out.push(
                new("bandwidth_change", Category::FrequencyIntegrity, Family::Lossy)
                    .sev(Severity::Moderate)
                    .ev(if strong { Evidence::Strong } else { Evidence::Weak }, if strong { 0.75 } else { 0.5 })
                    .text(
                        "Bandwidth changes abruptly",
                        "parts appear to come from a different source",
                        format!(
                            "The high-frequency ceiling jumps between {} and {} at {}. About {:.0} % of the track is limited to {}, consistent with {}.",
                            khz(first.from_hz),
                            khz(first.to_hz),
                            times_list(&times, 4),
                            share * 100.0,
                            khz(low),
                            label
                        ),
                    )
                    .why("Parts of the file seem to come from different sources, for example an edit assembled from a lossless and a lossy copy.", "The comparison only uses sections with similar overall level, so breakdowns are not mistaken for source changes. A steep filter automated in the production could look similar.")
                    .rec("Inspect the spectrogram around the marked times.")
                    .pen(p, p * 1.2, p)
                    .at(times),
            );
        }
    }

    fn artifacts(&self, ctx: &AssessContext, out: &mut Vec<Finding>) {
        let a = &ctx.m.artifacts;
        let Some(holes) = a.top_hole_ratio else { return };
        let lossy = is_lossy(ctx.facts);
        if holes >= 0.018 {
            let p = ((holes - 0.012) * 400.0).clamp(3.0, 15.0);
            let (ev, conf) = if lossy { (Evidence::Strong, 0.85) } else { (Evidence::Weak, 0.6) };
            out.push(
                new("spectral_holes", Category::CompressionArtifacts, Family::Lossy)
                    .sev(if holes >= 0.03 { Severity::Moderate } else { Severity::Low })
                    .ev(ev, conf)
                    .text(
                        if lossy { "Spectral holes from lossy coding" } else { "Spectral holes typical of lossy coding" },
                        "measurable compression artifacts",
                        format!(
                            "Between {} and {}, {} of spectral bins fall more than 20 dB below their neighbourhood. Natural, noise-like content shows about 0.7 %.",
                            khz(a.lo_hz),
                            khz(a.hi_hz),
                            pct(holes)
                        ),
                    )
                    .why("Lossy encoders save bits by zeroing coefficients they judge masked. The resulting holes can be heard as swirly, metallic or 'underwater' high frequencies, especially on cymbals and reverb.", if lossy {
                        "Strong indicator for this lossy file: the measurement matches the codec's behaviour."
                    } else {
                        "Weak indicator in a lossless file: sparse, tonal high-frequency content can also produce holes."
                    })
                    .pen(p, p, p * 1.2)
                    .metric("Hole ratio", pct(holes))
                    .metric("Range", format!("{}–{}", khz(a.lo_hz), khz(a.hi_hz))),
            );
        } else if lossy {
            out.push(
                new("no_measurable_artifacts", Category::CompressionArtifacts, Family::Lossy)
                    .ev(Evidence::Inconclusive, 0.5)
                    .text(
                        "No measurable compression artifacts",
                        "no measurable artifacts",
                        format!("Spectral holes stay at {} between {} and {}, close to natural content.", pct(holes), khz(a.lo_hz), khz(a.hi_hz)),
                    )
                    .why("Good encoders at high bitrates hide their work well.", "Inconclusive: the absence of measurable holes does not prove the encode is transparent."),
            );
        }
        // Side channel band-limited well below the full signal.
        if let (Some(se), Some(me)) = (ctx.m.bandwidth.side_edge.as_ref(), steep_edge(ctx)) {
            let side_level = ctx.m.stereo.as_ref().map(|s| s.side_to_mid_db).unwrap_or(-100.0);
            if se.is_steep() && se.hz < me.hz - 2000.0 && side_level > -30.0 {
                out.push(
                    new("side_band_limited", Category::CompressionArtifacts, Family::Lossy)
                        .sev(Severity::Low)
                        .ev(Evidence::Weak, 0.55)
                        .text(
                            "Stereo detail is band-limited",
                            "stereo highs band-limited",
                            format!("The side (L−R) signal stops at {} while the full signal reaches {}.", khz(se.hz), khz(me.hz)),
                        )
                        .why("Joint-stereo lossy coding at moderate bitrates often sacrifices high-frequency stereo information, narrowing the image.", "Weak indicator: some masters deliberately narrow the highs.")
                        .pen(4.0, 3.0, 5.0),
                );
            }
        }
    }

    fn resolution(&self, ctx: &AssessContext, out: &mut Vec<Finding>) {
        let m = ctx.m;
        if let (Some(eff), Some(cont)) = (m.effective_bits, m.container_bits) {
            if eff < cont && cont >= 24 && eff >= 16 {
                out.push(
                    new("padded_bits", Category::SourceIntegrity, Family::Resolution)
                        .sev(Severity::Low)
                        .text(
                            format!("{cont}-bit file with {eff}-bit content"),
                            format!("{eff}-bit audio padded to {cont} bits"),
                            format!("Only the top {eff} of {cont} bits carry data; the remaining bits are always zero."),
                        )
                        .why("Padding adds no resolution; the file is larger and may be presented as higher resolution than its source.", "Fact: measured over every decoded sample.")
                        .pen(3.0, 0.0, 0.0)
                        .metric("Effective bits", eff.to_string()),
                );
            } else if eff < 16 && cont >= 16 {
                out.push(
                    new("low_resolution", Category::NoiseArtifacts, Family::Resolution)
                        .sev(Severity::Moderate)
                        .text(
                            format!("Only {eff} bits of resolution"),
                            format!("{eff}-bit resolution"),
                            format!("Only the top {eff} of {cont} bits carry data."),
                        )
                        .why("Fewer bits mean a higher noise floor and coarser quiet passages.", "Fact: measured over every decoded sample.")
                        .pen(10.0, 8.0, 10.0),
                );
            }
        }
    }
}

impl Assessor for SourceRules {
    fn id(&self) -> &'static str {
        "source"
    }
    fn version(&self) -> &'static str {
        "1.0"
    }
    fn assess(&self, ctx: &AssessContext) -> Vec<Finding> {
        let mut out = Vec::new();
        if !has_signal(ctx) {
            return out;
        }
        if is_lossy(ctx.facts) {
            self.lossy_file(ctx, &mut out);
        } else if is_lossless(ctx.facts) {
            self.lossless_container(ctx, &mut out);
        } else {
            out.push(
                new("codec_unknown", Category::SourceIntegrity, Family::Lossy)
                    .ev(Evidence::Inconclusive, 0.4)
                    .text("Codec could not be identified", "unidentified codec", "The decoder could play the file, but its codec could not be classified as lossless or lossy.")
                    .why("Without the codec, source-quality conclusions rest on the signal alone.", "Inconclusive."),
            );
        }
        self.frequency(ctx, &mut out);
        self.artifacts(ctx, &mut out);
        self.resolution(ctx, &mut out);
        out
    }
}

// ---------------------------------------------------------------- clipping

struct ClippingRules;

impl Assessor for ClippingRules {
    fn id(&self) -> &'static str {
        "clipping"
    }
    fn version(&self) -> &'static str {
        "1.0"
    }
    fn assess(&self, ctx: &AssessContext) -> Vec<Finding> {
        let m = ctx.m;
        let mut out = Vec::new();
        if !has_signal(ctx) {
            return out;
        }
        if let Some(c) = m.clipping.as_ref().filter(|c| c.events > 0) {
            let epm = c.events_per_minute;
            let isolated = c.events <= 3 && c.longest_run <= 4;
            let (sev, p, label) = if isolated {
                (Severity::Low, (2.0, 3.0, 2.0), "isolated clipped samples")
            } else if epm >= 1000.0 || c.clipped_fraction >= 0.005 {
                (Severity::High, (35.0, 42.0, 35.0), "heavy hard clipping")
            } else if epm >= 300.0 {
                (Severity::High, (26.0, 32.0, 26.0), "frequent hard clipping")
            } else if epm >= 50.0 {
                (Severity::High, (18.0, 24.0, 18.0), "hard clipping")
            } else if epm >= 5.0 {
                (Severity::Moderate, (10.0, 14.0, 10.0), "hard clipping")
            } else {
                (Severity::Low, (5.0, 7.0, 5.0), "occasional hard clipping")
            };
            let ms = c.longest_run as f64 / m.sample_rate.max(1) as f64 * 1000.0;
            out.push(
                new("clipping", Category::ClippingDistortion, Family::Clipping)
                    .sev(sev)
                    .text(
                        if isolated { "Isolated clipped samples" } else { "Hard clipping detected" },
                        format!("{label} ({:.0}/min)", epm),
                        format!(
                            "{} flat-topped at {:.2} dBFS ({:.1} per minute, {} of all samples); the longest run is {} samples ({:.1} ms).",
                            plural(c.events, "peak", "peaks"),
                            c.ceiling_dbfs,
                            epm,
                            pct(c.clipped_fraction),
                            c.longest_run,
                            ms
                        ),
                    )
                    .why(
                        if isolated {
                            "A handful of very short clipped peaks is usually inaudible."
                        } else {
                            "Clipping adds harsh odd-order distortion. Some loud masters clip deliberately; on large club systems sustained clipping becomes fatiguing and cuts through the mix as crackle."
                        },
                        "Fact: measured as runs of identical samples at the channel's maximum level, which also catches clipping that was later normalised below 0 dBFS. Loudness itself is not penalised.",
                    )
                    .pen(p.0, p.1, p.2)
                    .metric("Clipped peaks", c.events.to_string())
                    .metric("Per minute", format!("{:.1}", epm))
                    .metric("Ceiling", format!("{:.2} dBFS", c.ceiling_dbfs)),
            );
        }
        if m.decoded_overs > 0 && m.peak_dbfs > 0.0 {
            let lossy = is_lossy(ctx.facts);
            let total = (m.duration * m.sample_rate as f64 * m.analyzed_channels.max(1) as f64).max(1.0);
            let frac = m.decoded_overs as f64 / total;
            let heavy = m.peak_dbfs > 0.5 || frac > 1e-4;
            out.push(
                new("decoded_overs", Category::ClippingDistortion, Family::Clipping)
                    .sev(if heavy { Severity::Moderate } else { Severity::Low })
                    .text(
                        "Samples above full scale",
                        format!("peaks reach +{:.2} dBFS", m.peak_dbfs),
                        format!(
                            "{} decode above 0 dBFS (peak +{:.2} dBFS).{}",
                            plural(m.decoded_overs, "sample", "samples"),
                            m.peak_dbfs,
                            if lossy { " Lossy decoders overshoot when the source was mastered right up to full scale." } else { " This floating-point file stores peaks above full scale." }
                        ),
                    )
                    .why("Fixed-point playback paths (many media players, CDJs, DAC inputs) clip these samples; floating-point DJ software is unaffected until its master output.", "Fact: measured from the decoded samples.")
                    .pen(if heavy { 5.0 } else { 2.0 }, if heavy { 8.0 } else { 3.0 }, if heavy { 4.0 } else { 2.0 })
                    .metric("Overs", m.decoded_overs.to_string())
                    .metric("Peak", format!("+{:.2} dBFS", m.peak_dbfs)),
            );
        }
        if m.true_peak_dbtp > 0.1 && m.peak_dbfs <= 0.0 {
            let high = m.true_peak_dbtp > 1.0;
            out.push(
                new("true_peak_overs", Category::ClippingDistortion, Family::Clipping)
                    .sev(if high { Severity::Moderate } else { Severity::Low })
                    .text(
                        "Inter-sample peaks above 0 dBTP",
                        format!("true peak +{:.1} dBTP", m.true_peak_dbtp),
                        format!(
                            "The reconstructed waveform peaks at +{:.2} dBTP between samples ({} over events; {}x oversampled, ITU-R BS.1770-4).",
                            m.true_peak_dbtp,
                            m.tp_over_events,
                            m.true_peak_oversampling
                        ),
                    )
                    .why("Converters and sample-rate converters can clip these peaks, and lossy re-encoding (streaming, recording a set) makes them worse.", "Fact: true peak measured with an oversampling meter. Common in loud modern masters and not itself a sign of poor quality.")
                    .pen(if high { 4.0 } else { 1.0 }, if high { 6.0 } else { 2.0 }, if high { 3.0 } else { 1.0 })
                    .metric("True peak", format!("+{:.2} dBTP", m.true_peak_dbtp)),
            );
        }
        out
    }
}

// --------------------------------------------------------- noise & stereo

struct NoiseStereoRules;

impl Assessor for NoiseStereoRules {
    fn id(&self) -> &'static str {
        "noise_stereo"
    }
    fn version(&self) -> &'static str {
        "1.0"
    }
    fn assess(&self, ctx: &AssessContext) -> Vec<Finding> {
        let m = ctx.m;
        let mut out = Vec::new();
        if !has_signal(ctx) {
            return out;
        }
        let dc = m.dc_offset.iter().cloned().map(f64::abs).fold(0.0, f64::max);
        if dc >= 0.005 {
            let strong = dc >= 0.02;
            out.push(
                new("dc_offset", Category::NoiseArtifacts, Family::Noise)
                    .sev(if strong { Severity::Moderate } else { Severity::Low })
                    .text(
                        "DC offset",
                        format!("DC offset {:.1} %", dc * 100.0),
                        format!("The waveform is shifted off centre by {:.2} % of full scale ({:.1} dBFS).", dc * 100.0, 20.0 * dc.log10()),
                    )
                    .why("DC offset wastes headroom and causes clicks when playback starts, stops or cuts.", "Fact: the mean of the decoded samples.")
                    .rec("A high-pass filter or DC-removal tool fixes this without affecting the sound.")
                    .pen(if strong { 6.0 } else { 3.0 }, if strong { 8.0 } else { 4.0 }, if strong { 4.0 } else { 2.0 }),
            );
        }
        let n = &m.noise;
        if n.quiet_seconds >= 1.0 {
            if let (Some(rms), Some(flat)) = (n.quiet_rms_db, n.quiet_hf_flatness) {
                if rms > -65.0 && flat >= 0.85 {
                    let loud = rms > -55.0;
                    out.push(
                        new("noise_floor", Category::NoiseArtifacts, Family::Noise)
                            .sev(if loud { Severity::Moderate } else { Severity::Low })
                            .ev(Evidence::Weak, 0.6)
                            .text(
                                "Audible noise floor",
                                format!("noise floor around {:.0} dBFS", rms),
                                format!("Quiet passages ({:.1} s) sit at {:.0} dBFS RMS with a flat, noise-like high-frequency spectrum (flatness {:.2}).", n.quiet_seconds, rms, flat),
                            )
                            .why("Broadband hiss is noticeable on headphones and in quiet intros or breakdowns.", "Weak indicator: measured only in quiet passages. Intentional textures (vinyl crackle, tape, field recordings) look the same.")
                            .pen(if loud { 8.0 } else { 4.0 }, if loud { 4.0 } else { 2.0 }, if loud { 10.0 } else { 5.0 })
                            .metric("Quiet level", format!("{:.0} dBFS", rms)),
                    );
                }
            }
        }
        if let Some(st) = &m.stereo {
            let low_loss = st.low_mono_loss_db.unwrap_or(0.0);
            let severe = st.correlation < -0.5 || low_loss < -10.0;
            let phase_issue = severe || st.correlation < -0.3 || (st.low_negative_fraction > 0.5 && st.low_correlation.unwrap_or(1.0) < -0.2) || low_loss < -6.0;
            if phase_issue {
                out.push(
                    new("phase_cancellation", Category::NoiseArtifacts, Family::Stereo)
                        .sev(if severe { Severity::High } else { Severity::Moderate })
                        .text(
                            "Channels are out of phase",
                            "bass cancels in mono",
                            format!(
                                "Left and right are largely out of phase (correlation {:.2}). Summed to mono, the low end changes by {:.0} dB and the whole mix by {:.0} dB.",
                                st.correlation, low_loss, st.mono_loss_db
                            ),
                        )
                        .why("Many club systems sum the bass to mono. Out-of-phase bass then cancels, and the kick and bassline vanish on the dance floor.", "Fact: measured correlation and mono fold-down of the decoded channels.")
                        .rec("If unintentional, invert the polarity of one channel. Check in mono before playing out.")
                        .pen(if severe { 20.0 } else { 10.0 }, if severe { 45.0 } else { 22.0 }, if severe { 12.0 } else { 6.0 })
                        .metric("Correlation", format!("{:.2}", st.correlation))
                        .metric("Bass in mono", format!("{:.0} dB", low_loss)),
                );
            } else if st.low_negative_fraction > 0.2 && st.low_correlation.is_some_and(|c| c < 0.5) {
                out.push(
                    new("bass_phase", Category::NoiseArtifacts, Family::Stereo)
                        .sev(Severity::Low)
                        .ev(Evidence::Weak, 0.5)
                        .text(
                            "Bass partly out of phase",
                            "bass phase issues in parts",
                            format!("In {:.0} % of bass-carrying passages the low end is out of phase between channels.", st.low_negative_fraction * 100.0),
                        )
                        .why("Out-of-phase bass loses energy on mono club systems.", "Weak indicator: wide stereo effects on low synths can be intentional.")
                        .pen(4.0, 10.0, 2.0),
                );
            }
            if st.identical {
                out.push(
                    new("dual_mono", Category::General, Family::Stereo)
                        .text("Mono content in a stereo file", "mono (identical channels)", "Both channels are sample-for-sample identical.")
                        .why("Not a defect: plays fine everywhere, just without stereo width.", "Fact."),
                );
            }
            if m.channel_rms_dbfs.len() >= 2 {
                let (a, b) = (m.channel_rms_dbfs[0], m.channel_rms_dbfs[1]);
                let diff = (a - b).abs();
                if a.min(b) < -80.0 && a.max(b) > -40.0 {
                    out.push(
                        new("silent_channel", Category::NoiseArtifacts, Family::Stereo)
                            .sev(Severity::High)
                            .text("One channel is silent", "one channel silent", format!("The {} channel is essentially silent ({:.0} dBFS RMS).", if a < b { "left" } else { "right" }, a.min(b)))
                            .why("Half the mix is missing on a stereo system.", "Fact.")
                            .pen(25.0, 35.0, 30.0),
                    );
                } else if diff > 6.0 {
                    out.push(
                        new("channel_imbalance", Category::NoiseArtifacts, Family::Stereo)
                            .sev(Severity::Low)
                            .ev(Evidence::Weak, 0.5)
                            .text("Channel level imbalance", format!("{:.1} dB channel imbalance", diff), format!("The {} channel is {:.1} dB louder on average.", if a > b { "left" } else { "right" }, diff))
                            .why("Unbalanced mixes pull the image to one side.", "Weak indicator: hard-panned arrangements can be intentional.")
                            .pen(4.0, 5.0, 4.0),
                    );
                }
            }
        }
        out
    }
}

// ----------------------------------------------------------------- context

struct ContextRules;

impl Assessor for ContextRules {
    fn id(&self) -> &'static str {
        "context"
    }
    fn version(&self) -> &'static str {
        "1.0"
    }
    fn assess(&self, ctx: &AssessContext) -> Vec<Finding> {
        let m = ctx.m;
        let mut out = Vec::new();
        if !has_signal(ctx) {
            return out;
        }
        if m.silence.leading >= 2.0 {
            out.push(
                new("leading_silence", Category::General, Family::Signal)
                    .text("Silence at the start", format!("{} of silence at the start", fmt_time(m.silence.leading)), format!("The first sound appears after {:.1} s.", m.silence.leading))
                    .why("Worth knowing when setting cue points.", "Measured: 10 ms blocks below -60 dBFS.")
                    .at(vec![m.silence.leading]),
            );
        }
        if m.silence.trailing >= 5.0 && m.silence.abrupt_cutoff.is_none() {
            out.push(
                new("trailing_silence", Category::General, Family::Signal)
                    .text("Long silence at the end", format!("{} of silence at the end", fmt_time(m.silence.trailing)), format!("The last {:.1} s are below -60 dBFS.", m.silence.trailing))
                    .why("Usually harmless; it can affect auto-mix and track length displays.", "Measured: 10 ms blocks below -60 dBFS."),
            );
        }
        if let Some(i) = m.integrated_lufs {
            if i < -20.0 {
                out.push(
                    new("quiet_master", Category::General, Family::Signal)
                        .text("Quiet master", format!("quiet master ({:.0} LUFS)", i), format!("Integrated loudness is {:.1} LUFS; typical club releases sit between -10 and -6 LUFS.", i))
                        .why("Not a quality problem: it simply needs more gain in a DJ set. Dynamic masters often sound better.", "Measured per ITU-R BS.1770-4."),
                );
            }
        }
        if m.duration < 30.0 {
            out.push(
                new("short_file", Category::General, Family::Signal)
                    .text("Short file", "short file", format!("Only {:.1} s of audio.", m.duration))
                    .why("Statistical measurements are less reliable on very short material.", "Fact."),
            );
        }
        out
    }
}

pub fn builtin() -> Vec<Box<dyn Assessor>> {
    vec![Box::new(IntegrityRules), Box::new(SourceRules), Box::new(ClippingRules), Box::new(NoiseStereoRules), Box::new(ContextRules)]
}

// --------------------------------------------------- confidence & verdict

pub fn confidence(ctx: &AssessContext, findings: &[Finding]) -> (f64, Vec<String>) {
    let m = ctx.m;
    let mut c: f64 = 0.95;
    let mut reasons = Vec::new();
    if m.silence.all_zero {
        return (0.95, vec!["The file is digital silence; there is nothing further to assess.".into()]);
    }
    if m.duration < 10.0 {
        c *= 0.5;
        reasons.push("Very short file: statistics rest on little material.".into());
    } else if m.duration < 30.0 {
        c *= 0.8;
        reasons.push("Short file: statistics rest on limited material.".into());
    }
    if m.truncated || m.decode_errors > 0 {
        c *= 0.85;
        reasons.push("Decoding was incomplete or reported errors, so not all audio could be measured.".into());
    }
    if findings.iter().any(|f| f.id == "source_inconclusive") {
        c *= 0.8;
        reasons.push("The original source quality could not be determined.".into());
    }
    if findings.iter().any(|f| f.id == "codec_unknown") {
        c *= 0.8;
        reasons.push("The codec could not be identified.".into());
    }
    // Penalties that rest on weak evidence lower confidence in proportion
    // to their share of all deductions.
    let total: f64 = findings.iter().map(|f| f.effective_penalty().overall).sum();
    if total > 0.0 {
        let weak: f64 = findings.iter().filter(|f| f.evidence == Evidence::Weak).map(|f| f.effective_penalty().overall).sum();
        let share = weak / total;
        if share > 0.0 {
            c *= 1.0 - 0.35 * share;
            reasons.push("Part of the score rests on weak indicators.".into());
        }
    }
    if reasons.is_empty() {
        reasons.push(if is_lossy(ctx.facts) {
            "Codec facts and direct measurements agree; nothing relies on weak evidence.".into()
        } else {
            "Conclusions rest on direct measurements and strong evidence.".into()
        });
    }
    (c.clamp(0.2, 0.98), reasons)
}

fn capitalize(s: &str) -> String {
    let mut ch = s.chars();
    match ch.next() {
        Some(f) => f.to_uppercase().collect::<String>() + ch.as_str(),
        None => String::new(),
    }
}

pub fn verdict(ctx: &AssessContext, findings: &[Finding], scores: &Scores) -> String {
    if findings.iter().any(|f| f.id == "silent_file") {
        return "Silent file: no audio to play or assess.".into();
    }
    let mut serious: Vec<&Finding> = findings
        .iter()
        .filter(|f| f.severity >= Severity::Moderate && f.evidence != Evidence::Inconclusive && f.effective_penalty().overall >= 4.0)
        .collect();
    serious.sort_by(|a, b| b.effective_penalty().overall.partial_cmp(&a.effective_penalty().overall).unwrap_or(std::cmp::Ordering::Equal));
    // Drop the generic codec finding when a more specific lossy finding exists.
    if serious.iter().any(|f| f.id == "bandwidth_below_bitrate") {
        serious.retain(|f| f.id != "lossy_codec");
    }
    // A cutoff explained by a lossy-source finding is not a separate headline.
    if serious.iter().any(|f| f.id == "lossy_source_suspected" || f.id == "lossy_codec" || f.id == "bandwidth_below_bitrate") {
        serious.retain(|f| f.id != "steep_cutoff");
    }
    let f = ctx.facts;
    if serious.is_empty() {
        // Low-severity facts worth naming in the headline.
        const NOTABLE: [&str; 5] = ["upsampled", "padded_bits", "extension_mismatch", "true_peak_overs", "decoded_overs"];
        let notable: Vec<String> = findings.iter().filter(|x| NOTABLE.contains(&x.id.as_str()) && !x.effective_penalty().is_zero()).take(2).map(|x| x.summary.clone()).collect();
        let minor = findings.iter().any(|x| x.id != "lossy_codec" && x.severity >= Severity::Low && !x.effective_penalty().is_zero());
        let base = if is_lossy(f) {
            match f.bitrate_kbps {
                Some(k) => format!("{} at {:.0} kbps", f.codec_name, k),
                None => f.codec_name.clone(),
            }
        } else {
            format!("Clean {} file", f.codec_name)
        };
        let lead = if is_lossy(f) && scores.overall >= 88.0 { format!("High-quality {base}") } else { base };
        if !notable.is_empty() {
            return format!("{lead}, but {}.", notable.join(" and "));
        }
        let tail = if findings.iter().any(|x| x.id == "source_inconclusive") {
            "; the original source quality cannot be fully verified."
        } else if minor {
            ". Only minor technical notes."
        } else if is_lossy(f) {
            ". No other technical issues detected."
        } else {
            ". No significant technical issues detected."
        };
        return format!("{lead}{tail}");
    }
    let phrases: Vec<String> = serious.iter().take(2).map(|f| f.summary.clone()).collect();
    let mut v = capitalize(&phrases.join("; "));
    if serious.iter().take(2).any(|f| f.evidence == Evidence::Weak) {
        v.push_str(". Manual review recommended.");
    } else {
        v.push('.');
    }
    v
}

pub fn flags(findings: &[Finding]) -> Vec<String> {
    let mut flags: Vec<String> = Vec::new();
    let mut add = |s: &str| {
        if !flags.iter().any(|f| f == s) {
            flags.push(s.to_string());
        }
    };
    for f in findings {
        let penalized = !f.effective_penalty().is_zero();
        match f.id.as_str() {
            "clipping" if f.severity >= Severity::Moderate => add("clipping"),
            "decoded_overs" | "true_peak_overs" if f.severity >= Severity::Moderate => add("clipping"),
            "lossy_codec" => {
                add("lossy");
                if f.severity >= Severity::High {
                    add("low_bitrate");
                }
            }
            "lossy_source_suspected" | "bandwidth_below_bitrate" | "bandwidth_change" => add("source_concern"),
            "spectral_holes" if f.severity >= Severity::Moderate => add("artifacts"),
            "extension_mismatch" => add("mislabeled"),
            "truncated" | "decode_errors" | "checksum_mismatch" | "dropouts" | "abrupt_ending" => add("integrity"),
            "phase_cancellation" | "silent_channel" | "bass_phase" => add("phase"),
            "channel_imbalance" if f.severity >= Severity::Low => add("phase"),
            "dc_offset" | "noise_floor" | "low_resolution" => add("noise"),
            "upsampled" => add("upsampled"),
            "padded_bits" => add("padded"),
            "silent_file" | "near_silent" => add("silent"),
            "source_inconclusive" | "codec_unknown" => add("inconclusive"),
            "steep_cutoff" => add("bandwidth"),
            _ => {}
        }
        if penalized && f.severity >= Severity::Moderate && matches!(f.evidence, Evidence::Fact | Evidence::Strong) {
            add("attention");
        }
        if f.evidence == Evidence::Weak && f.severity >= Severity::Moderate && penalized {
            add("review");
        }
    }
    flags
}
