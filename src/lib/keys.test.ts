import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  camelot,
  camelotDistance,
  camelotOrder,
  compatibleKeys,
  expandCompatible,
  formatBpm,
  formatKey,
  fromCamelot,
  keyHue,
  longKeyName,
  parseKey,
  WHEEL_ORDER,
} from './keys.ts';

const A_MINOR = 21;
const C_MAJOR = 0;

test('the Camelot wheel', () => {
  assert.deepEqual(camelot(C_MAJOR), { n: 8, letter: 'B' });
  assert.deepEqual(camelot(A_MINOR), { n: 8, letter: 'A' });
  assert.deepEqual(camelot(7), { n: 9, letter: 'B' }); // G major
  assert.deepEqual(camelot(11), { n: 1, letter: 'B' }); // B major
  assert.deepEqual(camelot(12 + 8), { n: 1, letter: 'A' }); // A♭ minor
  assert.deepEqual(camelot(12 + 1), { n: 12, letter: 'A' }); // D♭ minor
  for (let k = 0; k < 24; k += 1) {
    const { n, letter } = camelot(k);
    assert.equal(fromCamelot(n, letter === 'A'), k);
  }
  assert.equal(fromCamelot(0, true), null);
  assert.equal(fromCamelot(13, false), null);
});

test('every notation', () => {
  assert.equal(formatKey(A_MINOR, 'camelot'), '8A');
  assert.equal(formatKey(A_MINOR, 'openkey'), '1m');
  assert.equal(formatKey(C_MAJOR, 'openkey'), '1d');
  assert.equal(formatKey(12 + 8, 'musical'), 'A♭m');
  assert.equal(formatKey(12 + 8, 'sharps'), 'G♯m');
  assert.equal(formatKey(12 + 8, 'flats'), 'A♭m');
  assert.equal(formatKey(6, 'musical'), 'F♯');
  assert.equal(formatKey(6, 'flats'), 'G♭');
  assert.equal(formatKey(null, 'camelot'), '');
  assert.equal(formatKey(24, 'camelot'), '');
  assert.equal(longKeyName(1), 'D♭ major');
  assert.equal(longKeyName(A_MINOR), 'A minor');
});

test('keys are read in any notation', () => {
  const cases: [string, number | null][] = [
    ['Am', A_MINOR],
    ['A min', A_MINOR],
    ['a minor', A_MINOR],
    ['8A', A_MINOR],
    ['08a', A_MINOR],
    ['1m', A_MINOR],
    ['1d', C_MAJOR],
    ['C', C_MAJOR],
    ['Cmaj', C_MAJOR],
    ['F#m', 12 + 6],
    ['F♯m', 12 + 6],
    ['Gbm', 12 + 6],
    ['G♭', 6],
    ['Db', 1],
    ['B#', 0],
    ['Cb', 11],
    ['12B', 4],
    ['6d', 11], // Camelot 1B, B major
    ['', null],
    ['o', null],
    ['13A', null],
    ['0m', null],
    ['H', null],
    ['Am7', null],
  ];
  for (const [text, key] of cases) assert.equal(parseKey(text), key, text);
  for (let k = 0; k < 24; k += 1) {
    for (const n of ['camelot', 'openkey', 'musical', 'sharps', 'flats'] as const) assert.equal(parseKey(formatKey(k, n)), k, `${k} ${n}`);
  }
});

test('harmonic neighbours', () => {
  assert.deepEqual(compatibleKeys(A_MINOR).map((k) => formatKey(k, 'camelot')), ['8A', '8B', '9A', '7A']);
  assert.deepEqual(compatibleKeys(11).map((k) => formatKey(k, 'camelot')), ['1B', '1A', '2B', '12B']);
  assert.deepEqual(expandCompatible([A_MINOR, C_MAJOR]).map((k) => formatKey(k, 'camelot')), ['7A', '7B', '8A', '8B', '9A', '9B']);
  assert.equal(camelotDistance(A_MINOR, A_MINOR), 0);
  assert.equal(camelotDistance(A_MINOR, C_MAJOR), 1);
  assert.equal(camelotDistance(A_MINOR, parseKey('9A') as number), 1);
  assert.equal(camelotDistance(parseKey('1A') as number, parseKey('12A') as number), 1);
  assert.equal(camelotDistance(parseKey('1A') as number, parseKey('7B') as number), 7);
});

test('wheel order and hues', () => {
  assert.equal(WHEEL_ORDER.length, 24);
  assert.deepEqual(WHEEL_ORDER.slice(0, 4).map((k) => formatKey(k, 'camelot')), ['1A', '1B', '2A', '2B']);
  assert.ok(camelotOrder(parseKey('12B') as number) > camelotOrder(parseKey('12A') as number));
  // A key and its relative share a hue; neighbours are 30° apart.
  assert.equal(keyHue(A_MINOR), keyHue(C_MAJOR));
  assert.equal((keyHue(parseKey('9A') as number) - keyHue(A_MINOR) + 360) % 360, 30);
});

test('tempo', () => {
  assert.equal(formatBpm(124), '124');
  assert.equal(formatBpm(123.96), '124');
  assert.equal(formatBpm(127.54), '127.5');
  assert.equal(formatBpm(87.0004), '87');
  assert.equal(formatBpm(null), '');
  assert.equal(formatBpm(0), '');
});
