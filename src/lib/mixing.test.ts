import assert from 'node:assert/strict';
import { test } from 'node:test';
import { formatChange, keyMatch, mixStats, tempoChange } from './mixing.ts';

// The same cases as `keys_mix_by_the_wheel` in crates/sonora-core/src/playlists.rs.
test('keys mix by the Camelot wheel, as the core arranges them', () => {
  assert.equal(keyMatch(21, 21), 'same');
  assert.equal(keyMatch(21, 0), 'relative');
  assert.equal(keyMatch(21, 16), 'adjacent');
  assert.equal(keyMatch(21, 14), 'adjacent');
  assert.equal(keyMatch(21, 7), 'diagonal');
  assert.equal(keyMatch(21, 23), 'boost');
  assert.equal(keyMatch(21, 3), 'clash');
  assert.equal(keyMatch(null, 3), 'unknown');
  assert.equal(keyMatch(21, 24), 'unknown');
});

test('tempo changes consider half and double time', () => {
  const t = tempoChange(124, 126);
  assert.ok(t && Math.abs(t.pct - 1.6129) < 0.01 && !t.halfDouble);
  const d = tempoChange(87, 174);
  assert.ok(d && Math.abs(d.pct) < 1e-9 && d.halfDouble);
  assert.equal(tempoChange(null, 120), null);
  assert.equal(formatChange(1.6129), '+1.6%');
  assert.equal(formatChange(-0.44), '−0.4%');
  assert.equal(formatChange(0.01), '±0%');
  assert.equal(formatChange(12.4), '+12%');
});

test('a running order is summed up', () => {
  const s = mixStats([
    { djKey: 21, djBpm: 124 },
    { djKey: 16, djBpm: 126 },
    { djKey: 3, djBpm: 126 },
    { djKey: null, djBpm: null },
  ]);
  assert.deepEqual({ ...s, tempoChange: Math.round((s.tempoChange ?? 0) * 100) / 100 }, { transitions: 3, harmonic: 1, clashes: 1, unknown: 1, tempoChange: 0.81 });
});
