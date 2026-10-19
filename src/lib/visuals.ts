/**
 * Decoding of the compact visualization data stored with each analysis
 * (see crates/sonora-engine/src/analysis/mod.rs, `impl ToJson for Visuals`).
 */

import type { Visuals } from '../api/types.ts';
import { formatDb } from './format.ts';

export function base64ToBytes(b64: string): Uint8Array {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i += 1) out[i] = bin.charCodeAt(i);
  return out;
}

/** Inverse of the engine's `q_lin`: 0…255 → -1…1. */
export function dequantLinear(b: number): number {
  return b / 127.5 - 1;
}

/** Inverse of the engine's `q_db`: 0…255 → -90…0 dBFS. */
export function dequantDb(b: number): number {
  return (b / 255) * 90 - 90;
}

export interface WaveformData {
  count: number;
  bucketSecs: number;
  min: Float32Array;
  max: Float32Array;
  /** Linear band amplitudes (0…1) for colouring. */
  low: Float32Array;
  mid: Float32Array;
  high: Float32Array;
  rmsDb: Float32Array;
}

export function decodeWaveform(v: Visuals['waveform']): WaveformData {
  const n = v.count;
  const minB = base64ToBytes(v.min);
  const maxB = base64ToBytes(v.max);
  const rmsB = base64ToBytes(v.rms);
  const lowB = base64ToBytes(v.low);
  const midB = base64ToBytes(v.mid);
  const highB = base64ToBytes(v.high);
  const out: WaveformData = {
    count: n,
    bucketSecs: v.bucketSecs,
    min: new Float32Array(n),
    max: new Float32Array(n),
    low: new Float32Array(n),
    mid: new Float32Array(n),
    high: new Float32Array(n),
    rmsDb: new Float32Array(n),
  };
  const lin = (b: number | undefined) => (b == null || b === 0 ? 0 : 10 ** (dequantDb(b) / 20));
  for (let i = 0; i < n; i += 1) {
    out.min[i] = dequantLinear(minB[i] ?? 127.5);
    out.max[i] = dequantLinear(maxB[i] ?? 127.5);
    out.rmsDb[i] = dequantDb(rmsB[i] ?? 0);
    out.low[i] = lin(lowB[i]);
    out.mid[i] = lin(midB[i]);
    out.high[i] = lin(highB[i]);
  }
  return out;
}

export interface SpectrogramData {
  cols: number;
  rows: number;
  secsPerCol: number;
  maxHz: number;
  dbMin: number;
  dbMax: number;
  /** Column-major bytes, row 0 = 0 Hz. */
  data: Uint8Array;
}

export function decodeSpectrogram(v: Visuals['spectrogram']): SpectrogramData {
  return {
    cols: v.cols,
    rows: v.rows,
    secsPerCol: v.secsPerCol,
    maxHz: v.maxHz,
    dbMin: v.dbMin,
    dbMax: v.dbMax,
    data: base64ToBytes(v.data),
  };
}

/** dB value of one spectrogram cell. */
export function spectrogramDb(s: SpectrogramData, col: number, row: number): number {
  const b = s.data[col * s.rows + row] ?? 0;
  return s.dbMin + (b / 255) * (s.dbMax - s.dbMin);
}

type Stop = [number, number, number, number];

/**
 * Spectrogram colour map: graphite floor → deep steel → sage → pale lime.
 * Monotonic in lightness, so cutoffs read as hard luminance edges.
 */
const STOPS: Stop[] = [
  [0.0, 0x17, 0x19, 0x1c],
  [0.28, 0x1c, 0x2c, 0x3b],
  [0.48, 0x25, 0x4a, 0x5c],
  [0.66, 0x47, 0x76, 0x7a],
  [0.82, 0x8e, 0xae, 0x8f],
  [0.93, 0xc9, 0xdc, 0x9a],
  [1.0, 0xf1, 0xf6, 0xd8],
];

export function colormap(): Uint8ClampedArray {
  const lut = new Uint8ClampedArray(256 * 4);
  for (let i = 0; i < 256; i += 1) {
    const t = i / 255;
    let k = 0;
    while (k < STOPS.length - 2 && t > (STOPS[k + 1] as Stop)[0]) k += 1;
    const a = STOPS[k] as Stop;
    const b = STOPS[k + 1] as Stop;
    const f = b[0] === a[0] ? 0 : (t - a[0]) / (b[0] - a[0]);
    lut[i * 4] = a[1] + (b[1] - a[1]) * f;
    lut[i * 4 + 1] = a[2] + (b[2] - a[2]) * f;
    lut[i * 4 + 2] = a[3] + (b[3] - a[3]) * f;
    lut[i * 4 + 3] = 255;
  }
  return lut;
}

/**
 * Loudness timeline (see `Visuals['loudness']`): each value describes the
 * `window` seconds before a moment, as a loudness meter shows it, so value
 * `i` belongs at the end of its windows rather than at their start.
 */
export function loudnessWindow(v: Visuals['loudness'], duration: number): number {
  return v.window ?? (duration >= 3 ? 3 : 0.4);
}

/** Time (s) at which loudness value `i` is drawn. */
export function loudnessTime(i: number, step: number, window: number): number {
  return i * step + step / 2 + window;
}

/** Index of the loudness value shown at time `t`, or -1 before the first full window. */
export function loudnessIndexAt(t: number, step: number, window: number, count: number): number {
  if (count === 0 || t < window || !(step > 0)) return -1;
  return Math.max(0, Math.min(count - 1, Math.round((t - window - step / 2) / step)));
}

/**
 * Peak of the 8-bit waveform overview as text. Its resolution is about
 * 0.008 of full scale, so anything quieter than −40 dBFS (including digital
 * silence) reads as such rather than as a made-up exact level.
 */
export function formatOverviewPeak(peak: number): string {
  if (!(peak >= 0.01)) return 'below -40 dBFS';
  return `${formatDb(20 * Math.log10(Math.min(1, peak)), 0)} dBFS`;
}

/** Nice tick step for a span (1/2/5 × 10^n). */
export function niceStep(span: number, targetTicks: number): number {
  if (!(span > 0) || !(targetTicks > 0)) return 1;
  const raw = span / targetTicks;
  const mag = 10 ** Math.floor(Math.log10(raw));
  const norm = raw / mag;
  const nice = norm < 1.5 ? 1 : norm < 3.5 ? 2 : norm < 7.5 ? 5 : 10;
  return nice * mag;
}

/** Time-axis ticks in seconds (multiples of 5 s, 10 s, 30 s, 1 min…). */
export function timeTicks(duration: number, targetTicks: number): number[] {
  if (!(duration > 0)) return [];
  const steps = [1, 2, 5, 10, 15, 30, 60, 120, 300, 600, 900, 1800, 3600];
  const raw = duration / Math.max(1, targetTicks);
  const step = steps.find((s) => s >= raw) ?? 3600;
  const out: number[] = [];
  for (let t = 0; t <= duration + 1e-9; t += step) out.push(t);
  return out;
}
