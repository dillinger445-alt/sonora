import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  DASH,
  encodingShort,
  formatBytes,
  formatClock,
  formatCount,
  formatDb,
  formatDuration,
  formatEta,
  formatEtaShort,
  formatHz,
  formatRateKhz,
  formatRelative,
  formatSpan,
  formatTimecode,
  middleEllipsis,
  plural,
} from './format.ts';

test('durations and spans', () => {
  assert.equal(formatDuration(0), '0:00');
  assert.equal(formatDuration(372.4), '6:12');
  // B74: lengths round, as the lists show them (the player's clock floors).
  assert.equal(formatDuration(359.6), '6:00');
  assert.equal(formatDuration(3723), '1:02:03');
  assert.equal(formatDuration(null), DASH);
  assert.equal(formatDuration(-1), DASH);
  assert.equal(formatSpan(45), '45 s');
  assert.equal(formatSpan(14 * 60), '14 min');
  assert.equal(formatSpan(92 * 3600 + 14 * 60), '92 h 14 min');
  assert.equal(formatTimecode(65.25), '1:05');
  assert.equal(formatTimecode(65.25, true), '1:05.3');
});

test('levels keep their sign and never print -0', () => {
  assert.equal(formatDb(-8.42), '-8.4');
  assert.equal(formatDb(-0.04), '0.0');
  assert.equal(formatDb(0.84, 2, true), '+0.84');
  assert.equal(formatDb(0, 1, true), '0.0');
  assert.equal(formatDb(-250), '-inf');
  assert.equal(formatDb(Number.NaN), DASH);
});

test('frequencies, rates and sizes', () => {
  assert.equal(formatHz(16000), '16 kHz');
  assert.equal(formatHz(16800), '16.8 kHz');
  assert.equal(formatHz(880), '880 Hz');
  assert.equal(formatRateKhz(44100), '44.1');
  assert.equal(formatRateKhz(48000), '48');
  assert.equal(formatRateKhz(0), DASH);
  assert.equal(formatBytes(999), '999 B');
  assert.equal(formatBytes(38_200_000), '38.2 MB');
  assert.equal(formatBytes(1_500_000_000), '1.5 GB');
  // B56: rounded before the unit is chosen; fractions of a byte (speeds).
  assert.equal(formatBytes(999_600), '1.0 MB');
  assert.equal(formatBytes(999_700_000), '1.0 GB');
  assert.equal(formatBytes(99_960), '100 kB');
  assert.equal(formatBytes(523.4567891), '523 B');
  assert.equal(formatBytes(999.6), '1.0 kB');
  assert.equal(formatCount(1204), '1,204');
});

test('encoding shorthand distinguishes lossless and lossy', () => {
  assert.equal(encodingShort({ lossless: true, bitDepth: 16, sampleRate: 44100, bitrateKbps: 1411, format: 'FLAC' }), '16/44.1');
  assert.equal(encodingShort({ lossless: false, bitDepth: null, sampleRate: 44100, bitrateKbps: 320.4, format: 'MP3' }), '320k');
  assert.equal(encodingShort({ lossless: false, bitDepth: null, sampleRate: 48000, bitrateKbps: null, format: 'Opus' }), '48');
});

test('relative times, clocks and remaining time', () => {
  const now = new Date(2026, 9, 19, 15, 0, 0).getTime();
  assert.equal(formatRelative(now - 10_000, now), 'just now');
  assert.equal(formatRelative(now - 4 * 60_000, now), '4 min ago');
  assert.equal(formatRelative(now - 3 * 3600_000, now), '3 h ago');
  assert.equal(formatRelative(now - 30 * 3600_000, now), 'yesterday');
  assert.notEqual(formatClock(now - 60_000, now), DASH);
  assert.equal(formatEta(30_000), 'under a minute');
  assert.equal(formatEta(4 * 60_000), 'about 4 min');
  assert.equal(formatEtaShort(30_000), '<1 min');
  assert.equal(formatEtaShort(65 * 60_000), '1 h 5 min');
  // B75: never "60 min", "1 h 60 min" or "24 h ago".
  assert.equal(formatEta(7_170_000), 'about 2 h');
  assert.equal(formatEtaShort(7_170_000), '2 h');
  assert.equal(formatEta(3_590_000), 'about 1 h');
  assert.equal(formatEtaShort(3_590_000), '1 h');
  assert.equal(formatRelative(now - 3_590_000, now), '1 h ago');
  assert.equal(formatRelative(now - 85_000_000, now), 'yesterday');
  assert.equal(plural(1, 'track'), '1 track');
  assert.equal(plural(1204, 'track'), '1,204 tracks');
  assert.equal(plural(2, 'copy', 'copies'), '2 copies');
});

test('middle ellipsis keeps the file name', () => {
  const p = '/Users/alex/Music/Collections/Techno/2024/Label Name/Artist - Long Track Title (Extended Mix).flac';
  const short = middleEllipsis(p, 64);
  assert.ok(short.length <= 70, short);
  assert.ok(short.endsWith('Artist - Long Track Title (Extended Mix).flac'), short);
  assert.ok(short.includes('…'));
  assert.equal(middleEllipsis('/a/b.wav', 64), '/a/b.wav');
});
