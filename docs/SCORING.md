# How SONORA scores audio

This document describes the scoring model `sonora-dsp-1.1`: what is
measured, how measurements become findings, how findings become the three
scores, how confidence is estimated, and what the analysis can and cannot
detect. The code lives in `crates/sonora-engine/src/assess/` (model and
rules) and `crates/sonora-engine/src/analysis/` (measurements).

The model is deterministic DSP. No trained or machine-learning model is
involved, and the application does not claim otherwise.

## The three scores

Every analyzed track gets three scores from 0 to 100. They come from the
same findings, weighted differently:

| Score | Question it answers | What it weighs most | What it ignores |
| --- | --- | --- | --- |
| **Overall** | How good is this file, technically? | Encoding, source integrity, clipping, noise, frequency content | Taste |
| **DJ** | Can I play this on a club system? | Clipping and distortion, low bitrate and transcodes, dropouts, truncation, phase problems that collapse in mono | Loudness: a loud master is not penalised. Hi-res padding and upsampling do not count. |
| **Listening** | Is this good for attentive listening? | Audible artifacts, bandwidth, noise, distortion | Mastering choices such as loudness or a dark tonal balance |

The interface groups scores into bands. The bands are for display only and
do not affect the numbers.

| Band | Range | Colour |
| --- | --- | --- |
| Excellent | 90–100 | lime |
| Good | 75–89 | lime |
| Fair | 55–74 | amber |
| Poor | 35–54 | red |
| Very poor | 0–34 | red |

A score of 100 means that no measurable problem was found. It does not
certify that the recording or the mastering is good.

## Pipeline

1. **Decode.** WAV, AIFF and FLAC are decoded natively. Native FLAC decoding
   checks the stored MD5 against the decoded audio. Every other format
   (MP3, AAC/M4A, ALAC, Ogg Vorbis, Opus and others) is decoded by FFmpeg
   into a WAV stream on a pipe. The format is identified from the file
   content, never from its extension.
2. **Measure.** All meters run in one streaming pass, so memory use is
   bounded however long the file is. The result is a `Measurements` record
   (schema `MEASUREMENT_SCHEMA = 1`) plus visualization data.
3. **Assess.** The *assessors* (rule sets) turn facts and measurements into
   *findings*.
4. **Score.** Findings combine into sub-scores and the three headline
   scores. Confidence, a verdict sentence and filter flags are added.

Steps 3 and 4 read only the stored facts and measurements. When the scoring
model changes, SONORA re-scores existing analyses at startup without
decoding audio again (`library::rescore_outdated`). A test checks that a
stored analysis re-scores to exactly the same result.

## Measurements

| Area | Method |
| --- | --- |
| Peaks | Sample peak per channel, and decoded overs (samples beyond full scale, which lossy decoders and float files can produce). |
| True peak | ITU-R BS.1770-4 style: 4× oversampling below 88.2 kHz, 2× from 88.2 kHz up to (not including) 176.4 kHz, none at 176.4 kHz and above, with a 16-tap Kaiser-windowed sinc polyphase interpolator. Blocks that provably cannot exceed the running peak are skipped, and this shortcut is exact. Overs are counted as events at least 10 ms apart. |
| Loudness | EBU R128 / ITU-R BS.1770-4: K-weighting derived for any sample rate, 400 ms momentary and 3 s short-term blocks, absolute (−70 LUFS) and relative gates, integrated loudness and loudness range (EBU Tech 3342). Checked against FFmpeg's `ebur128` filter in the test suite. |
| Clipping | Lossless and float files: flat runs of at least 3 identical samples at the channel's own ceiling (within 0.03 dB), which also catches clipping that was later normalised below 0 dBFS. Events per minute, clipped samples, longest run. Not measured for lossy files, where decoding smooths flat runs away; their overs show up as decoded overs and true-peak overs instead. |
| Dynamics | RMS, crest factor, PLR (true peak − integrated loudness), DC offset per channel. |
| Spectrum | Long-term average spectrum of the channel average, (\|L\|² + \|R\|²) / 2, which unlike the mid signal cannot cancel out when one channel is inverted, and of the side signal (L − R). Energy share in nine bands from sub-bass to above 20 kHz. |
| Bandwidth | Upper limit (−50 dB below the 1–8 kHz level). Strongest spectral edge between 4 kHz and Nyquist, counted as a steep cutoff only when it drops ≥ 30 dB within ≤ 2 kHz (gentler edges are ordinary roll-off) and sits more than 1.5 kHz below Nyquist: an edge closer to it is the anti-alias filter every converter and resampler has (at 32 kHz just below 16 kHz), not an encoder's. Ceiling consistency over time, p99 breakthrough above the edge, side-channel edge, and for files above 48 kHz the ultrasonic level: the average level between 26 and 40 kHz relative to 2–20 kHz. |
| Compression artifacts | Share of "spectral holes" (bins far below their neighbourhood median) in short frames below the cutoff; spectral flatness. |
| Bandwidth over time | The cutoff in each 10 s segment, which reveals spliced sources. |
| Resolution | Effective bit depth from the OR of all integer samples, which catches 16-bit audio padded to 24 bits. Upsampling is detected from empty ultrasonic bands. |
| Silence and integrity | Leading and trailing silence; dropouts (exact digital zero on every channel, with loud audio on both sides); abrupt endings into zero fill; truncation (decoded length shorter than the header declares); decoder errors. |
| Noise | In quiet passages (−100 to −40 dBFS): their full-band RMS level, and the spectral flatness of their 4–16 kHz spectrum, which is close to 1 for broadband hiss. |
| Stereo | Correlation (full band and below 120 Hz), mono fold-down loss, side/mid ratio, identical or inverted channels. |
| Fingerprint | Sub-fingerprints at 10 per second from 33 log-spaced bands (300 Hz–3 kHz), following Haitsma & Kalker. Used for duplicates only, never for scoring. |

## Findings

Each rule that fires produces a **finding** with:

- **Category**: the sub-score it belongs to. The categories are source &
  encoding integrity, compression-artifact evidence, clipping & distortion,
  noise & unwanted artifacts, frequency integrity, and general (which is not
  scored).
- **Family**: its root cause, used to avoid double counting. The families
  are lossy, integrity, clipping, noise, stereo, resolution and signal. The
  symptoms of lossy coding (the codec itself, a cutoff, spliced bandwidth,
  spectral holes, a band-limited side channel) all belong to `lossy`.
- **Severity**: note, minor, moderate, serious or critical.
- **Evidence**:
  - **Confirmed (fact)**: measured directly or read from the file. Examples
    are the codec, clipped samples, a checksum mismatch and truncation.
  - **Strong**: several independent indicators agree.
  - **Weak**: one indicator, or a pattern with plausible innocent
    explanations. The interface recommends a listening check.
  - **Inconclusive**: the data cannot settle the question. These findings
    are shown but never deduct points.
- **Confidence** (0–1) and **nominal deductions** for each profile.
- **Text**: what was measured, why it matters, how reliable it is, and what
  to do.

**Effective deduction** = the nominal deduction for confirmed facts, the
nominal deduction × confidence for strong and weak evidence, and 0 for
inconclusive findings.

## Combining findings

**Headline scores.** Within each root-cause family, the largest effective
deduction counts in full and every further deduction counts at 25 %.
Family totals then add up, and the score is 100 minus the total, clamped to
0–100 and rounded:

```
family deduction = max(d) + 0.25 × (sum(d) − max(d))
score = clamp(100 − Σ family deductions, 0, 100)
```

This keeps one cause from being punished several times. For example, a
128 kbps MP3 has a lossy codec, a 16 kHz cutoff and spectral holes, which
are three symptoms of one decision. Independent defects such as clipping
plus a lossy source still add up.

**Sub-scores** use the same diminishing rule within each category, on the
overall-profile deductions, with a fixed 25 % weight. Each finding belongs
to one category, but one cause can produce findings in several: a 128 kbps
MP3 lowers Source & encoding (the codec), Frequency integrity (the cutoff)
and Compression artifacts (spectral holes). Sub-scores show where the
symptoms are; only the headline scores de-duplicate by root cause.

`ScoringConfig` exposes the weight for correlated findings, which applies to
the headline scores, and a scale for each profile. The defaults reproduce
`sonora-dsp-1.1`. Changing them is meant for future user settings; any
change must bump the model version.

## Rules

Deductions are listed as *overall / DJ / listening*. "Lossy table" refers to
the deduction for an MP3-equivalent bitrate, interpolated on a log scale:

| MP3-equivalent kbps | 64 | 96 | 128 | 160 | 192 | 224 | 256 | 320 | 448 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Overall deduction | 50 | 38 | 28 | 20 | 15 | 12 | 10 | 8 | 6 |

To get the MP3-equivalent rate, the bitrate is multiplied by an approximate
codec efficiency: AAC 1.3 (HE-AAC 1.4), Vorbis 1.25, Opus 1.5, MP2 0.75. It
is normalised to stereo.

### Integrity

| Finding | Evidence | Trigger | Deductions |
| --- | --- | --- | --- |
| `silent_file` | fact | Every sample is exactly zero | 100 / 100 / 100 |
| `near_silent` | fact | Never above −60 dBFS | 60 / 80 / 70 |
| `truncated` | fact | Decoded length shorter than the header declares | 45 / 60 / 50 |
| `decode_errors` | fact | Decoder reported errors | 10 + 5n / 12 + 6n / 10 + 5n (n ≤ 6) |
| `checksum_mismatch` | fact | FLAC MD5 differs from the decoded audio | 30 / 40 / 30 |
| `checksum_verified` | fact | FLAC MD5 matches | none (shown as a positive note) |
| `dropouts` | fact | Exact-zero gaps inside loud audio | 15 + 7(n−1), at most 40; ×1.4 DJ, ×1.1 listening |
| `abrupt_ending` | strong 0.85 | Loud audio stops into a long run of zeros | 35 / 50 / 35 |
| `extension_mismatch` | fact | Content format differs from the extension | 3 / 2 / 1 |

### Source and encoding

| Finding | Evidence | Trigger | Deductions |
| --- | --- | --- | --- |
| `lossy_codec` | fact | Lossy codec | Lossy table; ×1.25 DJ, ×1.1 listening. Severity: minor at ≥ 250 kbps equivalent, moderate at ≥ 160, serious below. With an unknown bitrate: 15 points, moderate. |
| `bandwidth_below_bitrate` | strong 0.85 (LAME header, or cutoff < 17.5 kHz) or weak 0.5 | Lossy file whose steep cutoff sits > 1 kHz below what the encoder keeps at that bitrate ("fake 320") | Lossy table at the bitrate implied by the cutoff |
| `lossy_source_suspected` | strong (0.75–0.95) or weak (0.4–0.6) | Lossless file with a steep cutoff. Points come from independent clues: cutoff frequency, nothing ever crossing the ceiling, a consistent ceiling, spectral holes, a side channel cut lower. Evidence is downgraded when little content reaches the cutoff region. | Lossy table at the implied bitrate + 4 |
| `source_inconclusive` | inconclusive | A cutoff that fits both a high-bitrate source and an ordinary filter, or too little high-frequency content to judge | none |
| `no_lossy_indicators` | inconclusive | Lossless file with full bandwidth | none |
| `steep_cutoff` | fact | Steep edge below 19.5 kHz | (19.5 kHz − cutoff) × 5 per kHz, between 2 and 25; ×1.1 listening |
| `bandwidth_change` | strong 0.75 (two or more segments) or weak 0.5 | The cutoff changes part-way through: a spliced source | Lossy table × affected share (at least 20 %) + 6; ×1.2 DJ |
| `spectral_holes` | strong 0.85 (lossy file) or weak 0.6 | ≥ 1.8 % holes below the cutoff (natural content measures about 0.7 %) | (ratio − 1.2 %) × 400, between 3 and 15; ×1.2 listening |
| `side_band_limited` | weak 0.55 | The side channel has a steep cutoff ≥ 2 kHz below the full signal's, and carries real content (above −30 dB relative to mid) | 4 / 3 / 5 (always minor) |
| `upsampled` | strong 0.85 | ≥ 88.2 kHz file whose content ends by 26 kHz, with the 26–40 kHz band ≤ −85 dB relative to 2–20 kHz | 6 / 0 / 0 |
| `padded_bits` | fact | 24-bit container, 16 to 23 effective bits | 3 / 0 / 0 |
| `low_resolution` | fact | Fewer than 16 effective bits | 10 / 8 / 10 |
| `codec_unknown` | inconclusive | Codec not identified | none (lowers confidence) |

### Clipping and distortion

| Finding | Evidence | Trigger | Deductions |
| --- | --- | --- | --- |
| `clipping` | fact | Isolated (≤ 3 events, runs ≤ 4 samples) | 2 / 3 / 2 |
| | | Fewer than 5 events per minute | 5 / 7 / 5 |
| | | ≥ 5 per minute | 10 / 14 / 10 |
| | | ≥ 50 per minute | 18 / 24 / 18 |
| | | ≥ 300 per minute | 26 / 32 / 26 |
| | | ≥ 1000 per minute or ≥ 0.5 % of samples | 35 / 42 / 35 |
| `decoded_overs` | fact | Decoded samples above full scale. Heavy above +0.5 dBFS or 0.01 % of samples. | 2 / 3 / 2; heavy 5 / 8 / 4 |
| `true_peak_overs` | fact | True peak above +0.1 dBTP while samples stay at or below 0 dBFS. High above +1 dBTP. | 1 / 2 / 1; high 4 / 6 / 3 |

Loudness is never penalised for its own sake.

### Noise and stereo

| Finding | Evidence | Trigger | Deductions |
| --- | --- | --- | --- |
| `dc_offset` | fact | ≥ 0.5 % (strong ≥ 2 %) | 3 / 4 / 2; strong 6 / 8 / 4 |
| `noise_floor` | weak 0.6 | At least 1 s of quiet passages whose level is above −65 dBFS (loud above −55) and whose 4–16 kHz spectrum is noise-like (flatness ≥ 0.85) | 4 / 2 / 5; loud 8 / 4 / 10 |
| `phase_cancellation` | fact | Any of: correlation below −0.3; the bass (below 120 Hz) loses more than 6 dB when folded to mono; more than half of the bass passages are out of phase with a bass correlation below −0.2. Severe when the correlation is below −0.5 or the bass loses more than 10 dB. | 10 / 22 / 6; severe 20 / 45 / 12 |
| `bass_phase` | weak 0.5 | Otherwise, more than 20 % of the bass passages out of phase with a bass correlation below 0.5 | 4 / 10 / 2 |
| `silent_channel` | fact | One channel below −80 dBFS RMS while the other is above −40 | 25 / 35 / 30 |
| `channel_imbalance` | weak 0.5 | More than 6 dB level difference | 4 / 5 / 4 |
| `dual_mono` | fact | Identical channels | none (note) |

DJ deductions for phase problems are higher because club systems often
sum the low end to mono.

### Context (notes, no deductions)

`leading_silence` (≥ 2 s), `trailing_silence` (≥ 5 s), `quiet_master`
(integrated loudness below −20 LUFS) and `short_file` (under 30 s) describe
the file without judging it.

## Confidence

Confidence starts at 0.95 and is multiplied down when the analysis rests on
less information:

| Condition | Factor |
| --- | --- |
| Under 10 s of audio | × 0.5 |
| Under 30 s | × 0.8 |
| Truncated or decoder errors | × 0.85 |
| Source quality inconclusive | × 0.8 |
| Unknown codec | × 0.8 |
| Weak-evidence findings, by their share *s* of all deductions | × (1 − 0.35 *s*) |

The result is clamped to 0.2–0.98 and labelled High (≥ 0.8), Medium (≥ 0.6)
or Low. The reasons are shown next to the verdict.

## Flags and "Needs attention"

Flags are short machine tags stored with each track for filtering:

| Flag | Raised by |
| --- | --- |
| `clipping` | `clipping`, `decoded_overs` or `true_peak_overs`, moderate or worse |
| `lossy` | `lossy_codec` (any lossy codec) |
| `low_bitrate` | A serious `lossy_codec` finding |
| `source_concern` | `lossy_source_suspected`, `bandwidth_below_bitrate` or `bandwidth_change` |
| `artifacts` | `spectral_holes`, moderate or worse |
| `integrity` | `truncated`, `decode_errors`, `checksum_mismatch`, `dropouts` or `abrupt_ending` |
| `mislabeled` | `extension_mismatch` |
| `phase` | `phase_cancellation`, `silent_channel`, `bass_phase` or `channel_imbalance` |
| `noise` | `dc_offset`, `noise_floor` or `low_resolution` |
| `upsampled` | `upsampled` |
| `padded` | `padded_bits` |
| `silent` | `silent_file` or `near_silent` |
| `inconclusive` | `source_inconclusive` or `codec_unknown` |
| `bandwidth` | `steep_cutoff` |

A track **needs attention** when a moderate-or-worse finding with a
deduction rests on confirmed or strong evidence (`attention`), or on weak
evidence (`review`, shown as "Worth a listen"). Tracks that failed to
analyze and missing files also appear in that view.

## What can and cannot be detected

**Reliable (confirmed facts):** codec, bitrate, sample rate and bit depth;
truncation; FLAC checksum mismatches; decoder errors; clipping in lossless
and float files, including clipping normalised below 0 dBFS; decoded overs;
true-peak overs; DC offset; dropouts to digital zero; silent or inverted
channels; extension mismatches; padded bit depth.

**Usually detected (strong evidence):** files transcoded from low- or
mid-bitrate lossy sources (about 192 kbps MP3 class and below); "fake 320"
MP3s re-encoded from lower bitrates, especially when the LAME header
records the encoder's own low-pass; spliced files whose bandwidth changes
part-way; upsampling to 88.2/96/192 kHz; zero-filled endings from
incomplete downloads.

**Hard or impossible to detect:**

- **High-bitrate lossy sources**: AAC 256 kbps, MP3 V0 and 320 kbps
  transcoded into FLAC or WAV keep (nearly) full bandwidth and leave no
  reliable trace. SONORA reports these as *inconclusive* rather than clean
  or fake. The corpus case `transcode_mp3_320.wav` scores 100 with *Medium*
  confidence and an explicit note.
- **Deliberate band-limiting**: a mastering low-pass or a steep resampler
  filter looks like a lossy cutoff. Without corroborating clues it is
  reported as *weak* evidence with a listening recommendation
  (`brickwall_16k.flac`).
- **Mains hum, buzz, clicks and crackle** (vinyl), **wow and flutter**, and
  **intersample distortion inside limiters** are not measured in this
  version.
- **Noise** is judged only in quiet passages, so it is weak evidence and
  can confuse intentional ambience with hiss.
- **Musical or mastering quality**, such as balance, loudness taste or
  dynamics choices, is not scored. Loudness and the loudness range are
  shown as facts only.

## Calibration

Scores on the generated test corpus (`sonora fixtures <dir>`, default
45-second tracks, this model version). Short files and short excerpts of
the same music can score slightly differently: the lossy decodes, for
example, overshoot full scale by different amounts.

| File | What it is | Overall | DJ | Listening | Confidence |
| --- | --- | --- | --- | --- | --- |
| clean_master.flac / .aiff / _16bit.wav / _24bit.wav, alac_16.m4a | Clean master in lossless containers | 100 | 100 | 100 | High |
| dark_master.flac | Naturally dark master (gentle roll-off) | 100 | 100 | 100 | High |
| mono_as_stereo.flac, other_track.flac | Identical channels; a second clean track | 100 | 100 | 100 | High |
| radio_edit.flac | 25-second edit of the clean master | 100 | 100 | 100 | Medium (short file) |
| transcode_mp3_320.wav | WAV made from a 320 kbps MP3 | 100 | 100 | 100 | Medium (inconclusive source) |
| padded_24bit.wav | 16-bit audio in a 24-bit container | 97 | 100 | 100 | High |
| upsampled_96k.flac | 44.1 kHz master upsampled to 96 kHz | 95 | 100 | 100 | High |
| noisy_hiss.wav | Hiss at about −50 dBFS | 95 | 98 | 94 | Medium |
| dc_offset.wav | 2 % DC offset | 94 | 92 | 96 | High |
| mp3_320.mp3 | MP3 320 kbps | 92 | 90 | 91 | High |
| mp3_v0.mp3 | MP3 V0 | 90 | 88 | 89 | High |
| actually_mp3.flac | MP3 with a .flac extension | 89 | 88 | 90 | High |
| opus_160.opus | Opus 160 kbps (one sample decodes above full scale) | 88 | 85 | 87 | High |
| aac_256.m4a | AAC 256 kbps (decodes up to +1.6 dBFS) | 87 | 83 | 88 | High |
| spliced_sources.flac | Half master, half 128 kbps MP3 | 85 | 82 | 84 | High |
| vorbis_q6.ogg | Vorbis q6 | 84 | 81 | 81 | High |
| polarity_inverted.wav | One channel inverted | 80 | 55 | 88 | High |
| brickwall_16k.flac | Master with a deliberate 16 kHz low-pass | 79 | 78 | 77 | Medium (weak evidence) |
| transcode_aac_128.flac | FLAC made from AAC 128 kbps | 78 | 72 | 76 | High |
| aac_128.m4a | AAC 128 kbps | 75 | 67 | 74 | High |
| dropouts.wav | Three 150 ms gaps | 71 | 59 | 68 | High |
| zero_filled_tail.wav | Incomplete download (zero fill) | 70 | 58 | 70 | High |
| transcode_mp3_128.flac | FLAC made from MP3 128 kbps | 69 | 62 | 66 | High |
| fake_320_from_128.mp3 | 320 kbps MP3 re-encoded from 128 kbps | 69 | 63 | 66 | High |
| mp3_128.mp3 | MP3 128 kbps | 67 | 60 | 64 | High |
| clipped_master.wav, clipped_normalized.flac | Heavy hard clipping (the second normalised to −0.3 dBFS) | 64 | 57 | 64 | High |
| truncated.flac | FLAC cut at 60 % | 55 | 40 | 50 | Medium (truncated, under 30 s) |
| silent.wav | Digital silence | 0 | 0 | 0 | High |
| corrupt.mp3, empty.flac, notes_renamed.wav | Not audio | failed with an explanation | | | |

The engine test suite (`crates/sonora-engine/tests/signals.rs`) covers the
cases the brief asked for: clean audio, clipping, silence, low-level noise
and deliberately restricted bandwidth. It also covers dropouts, polarity,
padding, upsampling (told apart from genuine 96 kHz content), truncation,
broken files and re-scoring determinism. The checks that need FFmpeg (real
lossy encodes and transcodes, clipping in a float WavPack file, and
loudness and true peak compared with FFmpeg's `ebur128` meter) are skipped
with a note when FFmpeg is not installed.

## Versioning and extending

- `SCORE_MODEL_VERSION` is stored with every result. Any change to rules,
  deductions or combination must bump it. Analyses scored by another
  version are refreshed from their stored measurements at startup.
- `MEASUREMENT_SCHEMA` versions the measurement record. A new measurement
  needs a schema bump, and old analyses then need re-analysis to gain it.
- New rule sets, including a future trained model, implement the
  `Assessor` trait (`id`, `version`, `assess(ctx) -> Vec<Finding>`) and
  return ordinary findings with honest evidence levels. A model's output
  would be reported as *strong* or *weak* evidence with its calibrated
  confidence, never as a confirmed fact.

## Changes

- **1.1**: an edge within 1.5 kHz of Nyquist is no longer a lossy cutoff
  (a lossless 32 kHz file was called a transcode); a sound too short for a
  spectrum frame gets no bandwidth finding; the "fake 320" detail names the
  bit rate and codec ("This 320 kbps MP3 file…"). Existing results are
  scored again from their stored measurements when SONORA starts.
