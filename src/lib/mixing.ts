/**
 * How neighbouring tracks mix: their keys on the Camelot wheel and their
 * tempo change. Mirrors `key_match` and `tempo_change` in
 * `crates/sonora-core/src/playlists.rs`, which arranges playlists by them.
 */

import { camelot } from './keys.ts';

export type KeyMatch = 'same' | 'relative' | 'adjacent' | 'diagonal' | 'boost' | 'clash' | 'unknown';

/** good: mixes cleanly; stretch: works with care; clash: keys fight. */
export type MixTone = 'good' | 'stretch' | 'clash' | 'unknown';

const isKey = (k: number | null | undefined): k is number => k != null && Number.isInteger(k) && k >= 0 && k < 24;

export function keyMatch(a: number | null | undefined, b: number | null | undefined): KeyMatch {
  if (!isKey(a) || !isKey(b)) return 'unknown';
  const ca = camelot(a);
  const cb = camelot(b);
  const d = (((ca.n - cb.n) % 12) + 12) % 12;
  const around = Math.min(d, 12 - d);
  const sameLetter = ca.letter === cb.letter;
  if (around === 0) return sameLetter ? 'same' : 'relative';
  if (around === 1) return sameLetter ? 'adjacent' : 'diagonal';
  if (around === 2 && sameLetter) return 'boost';
  return 'clash';
}

export function matchTone(m: KeyMatch): MixTone {
  switch (m) {
    case 'same':
    case 'relative':
    case 'adjacent':
      return 'good';
    case 'diagonal':
    case 'boost':
      return 'stretch';
    case 'clash':
      return 'clash';
    case 'unknown':
      return 'unknown';
  }
}

export const MATCH_LABEL: Record<KeyMatch, string> = {
  same: 'Same key',
  relative: 'Relative key (other mode)',
  adjacent: 'Neighbour on the wheel',
  diagonal: 'Neighbour, other mode',
  boost: 'Two steps round the wheel',
  clash: 'Keys clash',
  unknown: 'Key not known',
};

/**
 * Tempo change from `a` to `b` in percent, mixing at half or double time
 * when that is closer (`halfDouble`). `null` when a tempo is unknown.
 */
export function tempoChange(a: number | null | undefined, b: number | null | undefined): { pct: number; halfDouble: boolean } | null {
  if (a == null || b == null || !(a > 0) || !(b > 0)) return null;
  const direct = Math.log(b / a);
  const options: [number, boolean][] = [
    [direct, false],
    [direct + Math.LN2, true],
    [direct - Math.LN2, true],
  ];
  let best = options[0] as [number, boolean];
  for (const o of options) if (Math.abs(o[0]) < Math.abs(best[0])) best = o;
  return { pct: (Math.exp(best[0]) - 1) * 100, halfDouble: best[1] };
}

/** "+1.6%", "−0.4%", "±0%". */
export function formatChange(pct: number): string {
  const r = Math.round(pct * 10) / 10;
  if (r === 0) return '±0%';
  return `${r > 0 ? '+' : '−'}${Math.abs(r).toFixed(Math.abs(r) >= 10 ? 0 : 1)}%`;
}

export interface Transition {
  match: KeyMatch;
  tone: MixTone;
  tempo: { pct: number; halfDouble: boolean } | null;
}

export function transition(a: { djKey: number | null; djBpm: number | null }, b: { djKey: number | null; djBpm: number | null }): Transition {
  const match = keyMatch(a.djKey, b.djKey);
  return { match, tone: matchTone(match), tempo: tempoChange(a.djBpm, b.djBpm) };
}

export interface MixStats {
  transitions: number;
  harmonic: number;
  clashes: number;
  unknown: number;
  tempoChange: number | null;
}

export function mixStats(rows: { djKey: number | null; djBpm: number | null }[]): MixStats {
  const s: MixStats = { transitions: Math.max(0, rows.length - 1), harmonic: 0, clashes: 0, unknown: 0, tempoChange: null };
  let sum = 0;
  let n = 0;
  for (let i = 1; i < rows.length; i += 1) {
    const t = transition(rows[i - 1] as { djKey: number | null; djBpm: number | null }, rows[i] as { djKey: number | null; djBpm: number | null });
    if (t.tone === 'good') s.harmonic += 1;
    else if (t.tone === 'clash') s.clashes += 1;
    else if (t.tone === 'unknown') s.unknown += 1;
    if (t.tempo) {
      sum += Math.abs(t.tempo.pct);
      n += 1;
    }
  }
  s.tempoChange = n > 0 ? sum / n : null;
  return s;
}
