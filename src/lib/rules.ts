/**
 * Smart playlist rules in words, shared by a playlist's description and the
 * rules dialog, so both say the same about every rule (B44).
 */

import type { KeyNotation, LibraryFilter, SmartRules } from '../api/types.ts';
import { formatCount, plural } from './format.ts';
import { formatBpm, formatKey } from './keys.ts';

/** A saved score range in words: "overall score 55+", "DJ score 75–89". */
export function scoreRange(f: LibraryFilter): string | null {
  if (f.minScore == null && f.maxScore == null) return null;
  const kind = f.scoreKind === 'dj' ? 'DJ' : f.scoreKind === 'listening' ? 'listening' : 'overall';
  const lo = f.minScore != null ? Math.round(f.minScore) : null;
  // "up to 54.999" is how "below 55" is saved.
  const hi = f.maxScore != null ? Math.floor(f.maxScore) : null;
  if (lo != null && hi != null) return `${kind} score ${lo}–${hi}`;
  return lo != null ? `${kind} score ${lo}+` : `${kind} score up to ${hi}`;
}

/** Filters the rules dialog does not edit, in words. */
export function otherRules(f: LibraryFilter, withScore: boolean): string[] {
  const out: string[] = [];
  if (f.formats?.length) out.push(`format ${f.formats.join(', ')}`);
  if (f.flags?.length) out.push(f.flags.length <= 2 ? `issue ${f.flags.join(', ')}` : plural(f.flags.length, 'issue'));
  if (f.statuses?.length) out.push(`status ${f.statuses.join(', ')}`);
  if (f.folderId != null) out.push('one library folder');
  const range = withScore ? scoreRange(f) : null;
  if (range) out.push(range);
  if (f.attention) out.push('needs attention');
  return out;
}

/** The upper end of a tempo range as typed ("to 125" is saved as 125.999). */
export function bpmMaxText(max: number): string {
  return formatBpm(max % 1 > 0.99 ? Math.floor(max) : max);
}

/** "120–125 BPM; 8A 9A; House; “deep”; DJ score 75+; by tempo". */
export function describeRules(r: SmartRules | null | undefined, notation: KeyNotation): string {
  if (!r) return '';
  const f = r.filter;
  const parts: string[] = [];
  if (f.bpmMin != null || f.bpmMax != null) {
    parts.push(`${f.bpmMin != null ? formatBpm(f.bpmMin) : '…'}–${f.bpmMax != null ? bpmMaxText(f.bpmMax) : '…'} BPM${f.bpmHalfDouble ? ' (also half and double)' : ''}`);
  }
  if (f.keys?.length) parts.push(f.keys.length <= 4 ? f.keys.map((k) => formatKey(k, notation)).join(' ') : `${f.keys.length} keys`);
  if (f.genres?.length) parts.push(f.genres.join(', '));
  if (f.search) parts.push(`“${f.search}”`);
  parts.push(...otherRules(f, true));
  const sort: Record<string, string> = { bpm: 'tempo', key: 'key', title: 'title', artist: 'artist', dj: 'DJ score', overall: 'overall score', listening: 'listening score', added: 'date added' };
  const order = `by ${sort[r.sort.key] ?? r.sort.key}${r.sort.desc ? ', falling' : ''}`;
  const limit = r.limit ? `, at most ${formatCount(r.limit)}` : '';
  return `${parts.length ? parts.join('; ') : 'All tracks'}; ${order}${limit}`;
}

/** What "Sort into playlists" left out, in words (B45), or nothing. */
export function organizeLeftOut(r: { without: number; leftOut: number; missing: number }, by: 'genre' | 'key' | 'tempo'): string | undefined {
  const parts: string[] = [];
  if (r.without > 0) parts.push(`${plural(r.without, 'track')} without a ${by} stayed out.`);
  if (r.leftOut > 0) parts.push(`${plural(r.leftOut, 'more genre')} got no playlist: one folder holds at most 200.`);
  if (r.missing > 0) parts.push(`${plural(r.missing, 'track')} whose file is missing stayed out.`);
  return parts.length ? parts.join(' ') : undefined;
}
