import assert from 'node:assert/strict';
import { test } from 'node:test';
import { displayFlags, EVIDENCE, EVIDENCE_ORDER, flagInfo, scoreBand } from './scores.ts';

test('score bands', () => {
  assert.deepEqual(scoreBand(100), { band: 'good', label: 'Excellent' });
  assert.deepEqual(scoreBand(90), { band: 'good', label: 'Excellent' });
  assert.deepEqual(scoreBand(89.9), { band: 'good', label: 'Good' });
  assert.deepEqual(scoreBand(75), { band: 'good', label: 'Good' });
  assert.deepEqual(scoreBand(74), { band: 'fair', label: 'Fair' });
  assert.deepEqual(scoreBand(55), { band: 'fair', label: 'Fair' });
  assert.deepEqual(scoreBand(54), { band: 'poor', label: 'Poor' });
  assert.deepEqual(scoreBand(10), { band: 'poor', label: 'Very poor' });
  assert.equal(scoreBand(null), null);
  assert.equal(scoreBand(Number.NaN), null);
});

test('evidence levels are ordered from fact to inconclusive', () => {
  const levels = EVIDENCE_ORDER.map((e) => EVIDENCE[e].level);
  assert.deepEqual(levels, [4, 3, 2, 1]);
});

test('flags: specific problems first, explained or generic ones hidden', () => {
  // A steep cutoff is explained by the lossy source; "attention" is implied.
  assert.deepEqual(displayFlags(['attention', 'bandwidth', 'source_concern']), ['source_concern']);
  // "lossy" only shows when nothing more specific applies.
  assert.deepEqual(displayFlags(['lossy']), ['lossy']);
  assert.deepEqual(displayFlags(['lossy', 'low_bitrate', 'attention']), ['low_bitrate']);
  assert.deepEqual(displayFlags(['clipping', 'integrity', 'silent']), ['silent', 'integrity', 'clipping']);
  assert.deepEqual(displayFlags(['inconclusive']), ['inconclusive']);
  assert.deepEqual(displayFlags(['attention']), []);
  // A steep cutoff alone is worth showing.
  assert.deepEqual(displayFlags(['bandwidth']), ['bandwidth']);
  // Unknown flags from a newer core still show, by id.
  assert.deepEqual(displayFlags(['future_flag']), ['future_flag']);
  assert.equal(flagInfo('future_flag').label, 'future_flag');
});
