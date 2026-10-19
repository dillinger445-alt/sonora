import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  base64ToBytes,
  colormap,
  decodeSpectrogram,
  decodeWaveform,
  dequantDb,
  dequantLinear,
  formatOverviewPeak,
  loudnessIndexAt,
  loudnessTime,
  loudnessWindow,
  niceStep,
  spectrogramDb,
  timeTicks,
} from './visuals.ts';

const b64 = (bytes: number[]) => Buffer.from(bytes).toString('base64');

test('dequantization inverts the engine encoding', () => {
  assert.equal(dequantLinear(0), -1);
  assert.ok(Math.abs(dequantLinear(255) - 1) < 1e-9);
  assert.ok(Math.abs(dequantLinear(127.5)) < 1e-9);
  assert.equal(dequantDb(0), -90);
  assert.equal(dequantDb(255), 0);
  assert.deepEqual([...base64ToBytes(b64([0, 7, 255]))], [0, 7, 255]);
});

test('waveform decoding', () => {
  const w = decodeWaveform({ bucketSecs: 0.5, count: 2, min: b64([0, 127]), max: b64([255, 128]), rms: b64([255, 0]), low: b64([255, 0]), mid: b64([0, 0]), high: b64([0, 0]) });
  assert.equal(w.count, 2);
  assert.equal(w.min[0], -1);
  assert.ok(Math.abs((w.max[0] ?? 0) - 1) < 1e-6);
  assert.ok(Math.abs((w.low[0] ?? 0) - 1) < 1e-6, 'full-scale band is amplitude 1');
  assert.equal(w.low[1], 0, 'byte 0 is silence');
  assert.equal(w.rmsDb[1], -90);
});

test('spectrogram is column-major with row 0 at 0 Hz', () => {
  const s = decodeSpectrogram({ cols: 2, rows: 3, secsPerCol: 1, maxHz: 22050, dbMin: -140, dbMax: -10, data: b64([0, 0, 255, 255, 0, 0]) });
  assert.equal(spectrogramDb(s, 0, 2), -10);
  assert.equal(spectrogramDb(s, 1, 0), -10);
  assert.equal(spectrogramDb(s, 1, 2), -140);
});

test('colour map is monotonic in lightness', () => {
  const lut = colormap();
  const lum = (i: number) => 0.2126 * (lut[i * 4] ?? 0) + 0.7152 * (lut[i * 4 + 1] ?? 0) + 0.0722 * (lut[i * 4 + 2] ?? 0);
  for (let i = 1; i < 256; i += 1) assert.ok(lum(i) >= lum(i - 1) - 0.5, `step ${i}`);
});

test('axis ticks', () => {
  assert.equal(niceStep(100, 5), 20);
  assert.equal(niceStep(7, 7), 1);
  assert.deepEqual(timeTicks(90, 9), [0, 10, 20, 30, 40, 50, 60, 70, 80, 90]);
  assert.deepEqual(timeTicks(0, 5), []);
});

test('loudness values sit at the end of their window', () => {
  assert.equal(loudnessWindow({ step: 0.1, window: 3, values: [] }, 60), 3);
  // Analyses stored before the window was recorded.
  assert.equal(loudnessWindow({ step: 0.1, values: [] }, 60), 3);
  assert.equal(loudnessWindow({ step: 0.1, values: [] }, 2), 0.4);
  // The first short-term value covers 0–3 s.
  assert.ok(Math.abs(loudnessTime(0, 0.1, 3) - 3.05) < 1e-9);
  // Nothing to show before the first full window; then the matching value.
  assert.equal(loudnessIndexAt(2.5, 0.1, 3, 100), -1);
  assert.equal(loudnessIndexAt(loudnessTime(42, 0.1, 3), 0.1, 3, 100), 42);
  assert.equal(loudnessIndexAt(1e6, 0.1, 3, 100), 99);
});

test('overview peaks are not shown more precisely than they are stored', () => {
  // Digital silence decodes to 1/255 (byte 128), not to a real level.
  assert.equal(formatOverviewPeak(dequantLinear(128)), 'below -40 dBFS');
  assert.equal(formatOverviewPeak(1), '0 dBFS');
  assert.equal(formatOverviewPeak(0.5), '-6 dBFS');
});
