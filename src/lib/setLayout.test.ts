import assert from 'node:assert/strict';
import { test } from 'node:test';
import { columnsOf, layoutSet, segmentAt, selectedRuns } from './setLayout.ts';

const row = (id: number, duration: number | null = 300, djKey: number | null = (id % 24) + 1) => ({ id, displayTitle: `T${id}`, artist: null, duration, djBpm: 124, djKey });

test('a short set: blocks as long as their tracks, two pixels apart', () => {
  const segs = layoutSet([row(1, 300), row(2, 600), row(3, null)], 1000);
  assert.equal(segs.length, 3);
  assert.equal(segs[0]?.x, 0);
  assert.ok(Math.abs((segs[1]?.x ?? 0) - ((segs[0]?.w ?? 0) + 2)) < 1e-9);
  // An unknown length counts as four minutes.
  assert.ok(Math.abs((segs[2]?.w ?? 0) / (segs[0]?.w ?? 1) - 240 / 300) < 1e-9);
  assert.equal(segs[0]?.t, null);
  assert.ok(segs[1]?.t);
});

test('a set of thousands fits the width (it used to run far past it)', () => {
  const rows = Array.from({ length: 5000 }, (_, i) => row(i + 1));
  const segs = layoutSet(rows, 1100);
  const last = segs.at(-1);
  assert.ok(last && last.x + last.w <= 1100 + 1e-6, `ends at ${last?.x} + ${last?.w}`);
  // A pixel column per colour run, never more than the width.
  const cols = columnsOf(segs, 1100);
  assert.ok(cols.length <= 1100 && cols.length > 100);
  assert.equal(cols.reduce((a, c) => a + c.w, 0), 1100);
});

test('the pointer finds the block under it, gaps belonging to the block before', () => {
  const segs = layoutSet([row(1, 100), row(2, 100), row(3, 100)], 302);
  assert.equal(segmentAt(segs, -1), null);
  assert.equal(segmentAt(segs, 0), 0);
  assert.equal(segmentAt(segs, 98), 0);
  assert.equal(segmentAt(segs, 101), 0);
  assert.equal(segmentAt(segs, 103), 1);
  assert.equal(segmentAt(segs, 301), 2);
  assert.equal(segmentAt(segs, 400), null);
  assert.equal(segmentAt([], 5), null);
});

test('selected tracks: one outline each, or one per run in a long set', () => {
  const segs = layoutSet([row(1), row(2), row(3), row(4)], 400);
  const sel = new Set([1, 2, 4]);
  assert.equal(selectedRuns(segs, sel, false).length, 3);
  const runs = selectedRuns(segs, sel, true);
  assert.equal(runs.length, 2);
  assert.ok(Math.abs((runs[0]?.w ?? 0) - ((segs[1]?.x ?? 0) + (segs[1]?.w ?? 0))) < 1e-9);
});
