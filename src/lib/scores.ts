/**
 * Presentation rules for scores, findings and flags. Band boundaries are
 * documented in docs/SCORING.md; the scores themselves come from the core.
 */

import type { Evidence, FlagId, ScoreKind, Severity } from '../api/types.ts';

export type Band = 'good' | 'fair' | 'poor';

export interface BandInfo {
  band: Band;
  label: string;
}

/** 90+ excellent, 75+ good, 55+ fair, 35+ poor, below that very poor. */
export function scoreBand(score: number | null | undefined): BandInfo | null {
  if (score == null || !Number.isFinite(score)) return null;
  if (score >= 90) return { band: 'good', label: 'Excellent' };
  if (score >= 75) return { band: 'good', label: 'Good' };
  if (score >= 55) return { band: 'fair', label: 'Fair' };
  if (score >= 35) return { band: 'poor', label: 'Poor' };
  return { band: 'poor', label: 'Very poor' };
}

export const SCORE_KINDS: ScoreKind[] = ['overall', 'dj', 'listening'];

export const SCORE_LABEL: Record<ScoreKind, string> = {
  overall: 'Overall',
  dj: 'DJ',
  listening: 'Listening',
};

export const SCORE_HINT: Record<ScoreKind, string> = {
  overall: 'Technical quality of the file: encoding, source integrity, clipping, noise and frequency content.',
  dj: 'Fitness for club playback: clipping and distortion, encoding artifacts, source integrity and reliability. Loudness is not penalised.',
  listening: 'Fitness for attentive listening: audible artifacts, bandwidth, noise and distortion. Mastering taste is not judged.',
};

export interface EvidenceInfo {
  label: string;
  /** 1–4 filled pips. */
  level: number;
  description: string;
}

export const EVIDENCE: Record<Evidence, EvidenceInfo> = {
  fact: { label: 'Confirmed', level: 4, description: 'Measured directly or read from the file. Not an inference.' },
  strong: { label: 'Strong evidence', level: 3, description: 'Several independent measurements agree; false positives are rare.' },
  weak: { label: 'Weak evidence', level: 2, description: 'A plausible reading of the measurements, but other explanations exist. Check by ear.' },
  inconclusive: { label: 'Inconclusive', level: 1, description: 'The measurements cannot settle this either way.' },
};

export const EVIDENCE_ORDER: Evidence[] = ['fact', 'strong', 'weak', 'inconclusive'];

export interface SeverityInfo {
  label: string;
  tone: 'neutral' | 'amber' | 'red';
  rank: number;
}

export const SEVERITY: Record<Severity, SeverityInfo> = {
  info: { label: 'Note', tone: 'neutral', rank: 0 },
  low: { label: 'Minor', tone: 'neutral', rank: 1 },
  moderate: { label: 'Moderate', tone: 'amber', rank: 2 },
  high: { label: 'Serious', tone: 'red', rank: 3 },
  critical: { label: 'Critical', tone: 'red', rank: 4 },
};

export interface FlagInfo {
  label: string;
  tone: 'neutral' | 'amber' | 'red';
  description: string;
}

export const FLAGS: Record<FlagId, FlagInfo> = {
  attention: { label: 'Needs attention', tone: 'amber', description: 'A significant problem backed by confirmed or strong evidence.' },
  review: { label: 'Worth a listen', tone: 'amber', description: 'A possible problem with weak evidence; check by ear.' },
  clipping: { label: 'Clipping', tone: 'red', description: 'Clipped samples or decoded overs.' },
  lossy: { label: 'Lossy', tone: 'neutral', description: 'Encoded with a lossy codec (MP3, AAC, Vorbis, Opus…).' },
  low_bitrate: { label: 'Low bitrate', tone: 'amber', description: 'Lossy encoding at a bitrate where compression artifacts are usually audible.' },
  source_concern: { label: 'Lossy source?', tone: 'amber', description: 'Signs that the audio passed through a lossy encoder before this file was made.' },
  artifacts: { label: 'Artifacts', tone: 'amber', description: 'Measurable lossy-compression artifacts, such as spectral holes or a band-limited stereo image.' },
  integrity: { label: 'Integrity', tone: 'red', description: 'Truncation, decode errors, checksum mismatch, dropouts or an abrupt ending.' },
  mislabeled: { label: 'Wrong extension', tone: 'amber', description: 'The file name extension does not match the audio format inside the file.' },
  phase: { label: 'Stereo/phase', tone: 'amber', description: 'Phase cancellation, an inverted or silent channel, out-of-phase bass or a channel imbalance.' },
  noise: { label: 'Noise', tone: 'amber', description: 'DC offset, a raised noise floor or reduced effective resolution.' },
  upsampled: { label: 'Upsampled', tone: 'neutral', description: 'No content above the original band limit: the sample rate was raised after recording.' },
  padded: { label: 'Padded bits', tone: 'neutral', description: 'Fewer effective bits than the container declares.' },
  silent: { label: 'Silent', tone: 'red', description: 'Silent or nearly silent file.' },
  inconclusive: { label: 'Inconclusive', tone: 'neutral', description: 'Some checks could not reach a conclusion.' },
  bandwidth: { label: 'Steep cutoff', tone: 'neutral', description: 'A sharp high-frequency cutoff below the Nyquist limit.' },
};

/** Flags worth showing in compact lists, most important first. */
export const FLAG_PRIORITY: FlagId[] = [
  'silent',
  'integrity',
  'clipping',
  'source_concern',
  'low_bitrate',
  'artifacts',
  'mislabeled',
  'phase',
  'noise',
  'review',
  'upsampled',
  'padded',
  'bandwidth',
  'lossy',
  'inconclusive',
];

export function flagInfo(id: string): FlagInfo {
  return (FLAGS as Record<string, FlagInfo>)[id] ?? { label: id, tone: 'neutral', description: '' };
}

/**
 * Flags worth showing, most important first. `attention` is implied by the
 * others; a steep cutoff is explained by a lossy encoding when there is one;
 * "lossy" and "inconclusive" only show when nothing more specific applies.
 */
export function displayFlags(flags: string[]): string[] {
  const set = new Set(flags);
  set.delete('attention');
  if (set.has('lossy') || set.has('source_concern') || set.has('low_bitrate')) set.delete('bandwidth');
  const known = FLAG_PRIORITY.filter((f) => set.has(f));
  const unknown = [...set].filter((f) => !FLAG_PRIORITY.includes(f as FlagId));
  const ordered = [...known, ...unknown];
  const specific = ordered.filter((f) => f !== 'lossy' && f !== 'inconclusive');
  return specific.length > 0 ? specific : ordered;
}

export function confidenceTone(label: string | null | undefined): 'good' | 'fair' | 'poor' | null {
  if (label === 'High') return 'good';
  if (label === 'Medium') return 'fair';
  if (label === 'Low') return 'poor';
  return null;
}
