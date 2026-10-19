/**
 * Musical keys as the core numbers them (0–11 major, 12–23 minor; the tonic
 * is `key % 12` with 0 = C) and the notations DJs use for them. Mirrors
 * `sonora_engine::analysis::key`, so the interface and the file tags agree.
 */

import type { KeyNotation } from '../api/types.ts';

/** Spellings of the Camelot wheel (Mixed In Key): flats except F♯. */
const MUSICAL = ['C', 'D♭', 'D', 'E♭', 'E', 'F', 'F♯', 'G', 'A♭', 'A', 'B♭', 'B'];
const SHARPS = ['C', 'C♯', 'D', 'D♯', 'E', 'F', 'F♯', 'G', 'G♯', 'A', 'A♯', 'B'];
const FLATS = ['C', 'D♭', 'D', 'E♭', 'E', 'F', 'G♭', 'G', 'A♭', 'A', 'B♭', 'B'];

export const KEY_NOTATIONS: { id: KeyNotation; label: string; example: string; description: string }[] = [
  { id: 'camelot', label: 'Camelot', example: '8A', description: 'The Camelot wheel of Mixed In Key: 1A–12A minor, 1B–12B major' },
  { id: 'openkey', label: 'Open Key', example: '1m', description: 'Open Key, as in Traktor: 1m–12m minor, 1d–12d major' },
  { id: 'musical', label: 'Musical', example: 'Am', description: 'Note names as on the Camelot wheel: A♭m, F♯m, D♭' },
  { id: 'sharps', label: 'Sharps', example: 'G♯m', description: 'Note names with sharps: C♯, D♯, F♯, G♯, A♯' },
  { id: 'flats', label: 'Flats', example: 'A♭m', description: 'Note names with flats: D♭, E♭, G♭, A♭, B♭' },
];

export function isNotation(v: unknown): v is KeyNotation {
  return typeof v === 'string' && KEY_NOTATIONS.some((n) => n.id === v);
}

export function isMinor(key: number): boolean {
  return key >= 12;
}

export function tonic(key: number): number {
  return key % 12;
}

/** Camelot number (1–12) and letter ('A' minor, 'B' major). */
export function camelot(key: number): { n: number; letter: 'A' | 'B' } {
  // One step round the wheel is a fifth; C major is 8B and every minor key
  // shares its number with its relative major.
  const majorTonic = isMinor(key) ? (tonic(key) + 3) % 12 : tonic(key);
  return { n: (((majorTonic * 7) % 12) + 7) % 12 + 1, letter: isMinor(key) ? 'A' : 'B' };
}

/** The key with Camelot number `n` (1–12), or `null`. */
export function fromCamelot(n: number, minor: boolean): number | null {
  if (!Number.isInteger(n) || n < 1 || n > 12) return null;
  // 7 is its own inverse modulo 12.
  const majorTonic = (((n + 12 - 8) % 12) * 7) % 12;
  return minor ? 12 + ((majorTonic + 9) % 12) : majorTonic;
}

function noteName(key: number, names: string[]): string {
  const base = names[tonic(key)] ?? '?';
  return isMinor(key) ? `${base}m` : base;
}

/** A key in the given notation: "8A", "1m", "A♭m", "G♯m". */
export function formatKey(key: number | null | undefined, notation: KeyNotation): string {
  if (key == null || !Number.isInteger(key) || key < 0 || key > 23) return '';
  const { n, letter } = camelot(key);
  switch (notation) {
    case 'camelot':
      return `${n}${letter}`;
    case 'openkey':
      return `${((n + 12 - 8) % 12) + 1}${isMinor(key) ? 'm' : 'd'}`;
    case 'musical':
      return noteName(key, MUSICAL);
    case 'sharps':
      return noteName(key, SHARPS);
    case 'flats':
      return noteName(key, FLATS);
  }
}

/** "A minor", "D♭ major". */
export function longKeyName(key: number): string {
  return `${MUSICAL[tonic(key)] ?? '?'} ${isMinor(key) ? 'minor' : 'major'}`;
}

/**
 * Reads a key written in any common notation: "Am", "A min", "A minor",
 * "F#m", "G♭", "Dbmaj", "8A", "08b", "1m", "6d". `null` for anything else.
 */
export function parseKey(text: string): number | null {
  const t = text.trim().replace(/♯/g, '#').replace(/♭/g, 'b').replace(/[\s_-]/g, '');
  if (!t) return null;
  const lower = t.toLowerCase();
  const num = /^(\d+)([a-z]*)$/.exec(lower);
  if (num) {
    const n = Number(num[1]);
    const rest = num[2];
    if (rest === 'a' || rest === 'b') return fromCamelot(n, rest === 'a');
    if (rest === 'm' || rest === 'd') {
      if (n < 1 || n > 12) return null;
      return fromCamelot(((n + 6) % 12) + 1, rest === 'm');
    }
    return null;
  }
  const base = { c: 0, d: 2, e: 4, f: 5, g: 7, a: 9, b: 11 }[lower[0] ?? ''];
  if (base == null) return null;
  let rest = lower.slice(1);
  let pc = base;
  // Accidentals come first; no mode word starts with "b".
  for (;;) {
    if (rest.startsWith('#')) pc += 1;
    else if (rest.startsWith('b')) pc -= 1;
    else break;
    rest = rest.slice(1);
  }
  let minor: boolean;
  if (['', 'maj', 'major', 'dur'].includes(rest)) minor = false;
  else if (['m', 'min', 'minor', 'moll'].includes(rest)) minor = true;
  else return null;
  const p = ((pc % 12) + 12) % 12;
  return minor ? 12 + p : p;
}

/** Keys that mix harmonically with `key`: itself, its relative and its neighbours on the wheel. */
export function compatibleKeys(key: number): number[] {
  const { n, letter } = camelot(key);
  const minor = letter === 'A';
  const up = (n % 12) + 1;
  const down = ((n + 10) % 12) + 1;
  return [fromCamelot(n, minor), fromCamelot(n, !minor), fromCamelot(up, minor), fromCamelot(down, minor)].filter((k): k is number => k != null);
}

/** Every key that mixes with at least one of `keys`, in wheel order. */
export function expandCompatible(keys: number[]): number[] {
  const out = new Set<number>();
  for (const k of keys) for (const c of compatibleKeys(k)) out.add(c);
  return [...out].sort((a, b) => camelotOrder(a) - camelotOrder(b));
}

/** Steps between two keys on the wheel: 0 same key, 1 for a neighbour or the relative key. */
export function camelotDistance(a: number, b: number): number {
  const ca = camelot(a);
  const cb = camelot(b);
  const d = (((ca.n - cb.n) % 12) + 12) % 12;
  return Math.min(d, 12 - d) + (ca.letter === cb.letter ? 0 : 1);
}

/** Wheel order for sorting: by number, minor (A) before major (B). */
export function camelotOrder(key: number): number {
  const { n, letter } = camelot(key);
  return n * 2 + (letter === 'B' ? 1 : 0);
}

/** All 24 keys in wheel order (1A, 1B, 2A, …). */
export const WHEEL_ORDER: number[] = Array.from({ length: 24 }, (_, i) => i).sort((a, b) => camelotOrder(a) - camelotOrder(b));

/**
 * A muted hue for a key's position on the wheel (neighbours get
 * neighbouring hues, as on printed Camelot wheels). Low chroma, so it never
 * reads as a status colour.
 */
export function keyHue(key: number): number {
  return ((camelot(key).n - 1) * 30 + 95) % 360;
}

/** The colour of a key's place on the wheel (for small marks only). */
export function keyColor(key: number): string {
  return `hsl(${keyHue(key)} 36% 64%)`;
}

/** Tempo as DJs read it: whole numbers without decimals, others with one ("127.5"). */
export function formatBpm(bpm: number | null | undefined): string {
  if (bpm == null || !Number.isFinite(bpm) || bpm <= 0) return '';
  const r = Math.round(bpm * 10) / 10;
  return Number.isInteger(r) ? String(r) : r.toFixed(1);
}
