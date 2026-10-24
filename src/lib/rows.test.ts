import assert from 'node:assert/strict';
import { test } from 'node:test';
import { reuseRows } from './rows.ts';

test('rows fetched again keep the objects of unchanged tracks', () => {
  const a = { id: 1, title: 'One', flags: ['clipping'] };
  const b = { id: 2, title: 'Two', flags: [] as string[] };
  const fresh = [
    { id: 2, title: 'Two', flags: [] as string[] },
    { id: 1, title: 'One (edited)', flags: ['clipping'] },
    { id: 3, title: 'Three', flags: [] as string[] },
  ];
  const out = reuseRows([a, b], fresh);
  // Moved but unchanged: the old object, in its new place.
  assert.equal(out[0], b);
  // Changed or new: the fresh objects.
  assert.equal(out[1], fresh[1]);
  assert.equal(out[2], fresh[2]);
  assert.deepEqual(reuseRows([], fresh), fresh);
});
