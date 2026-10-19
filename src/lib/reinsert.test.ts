import assert from 'node:assert/strict';
import { test } from 'node:test';
import { reinsertions, type Reinsertion } from './reinsert.ts';

/** The list after applying the insertions, as the core does. */
function apply(current: number[], steps: Reinsertion[]): number[] {
  const list = [...current];
  for (const s of steps) list.splice(Math.min(s.index, list.length), 0, ...s.ids);
  return list;
}

test('undo puts tracks back where they were', () => {
  const before = [1, 2, 3, 4, 5];
  assert.deepEqual(apply([1, 3, 4, 5], reinsertions(before, [2], [1, 3, 4, 5])), before);
  assert.deepEqual(apply([3, 5], reinsertions(before, [1, 2, 4], [3, 5])), before);
  assert.deepEqual(apply([1, 2, 3], reinsertions(before, [4, 5], [1, 2, 3])), before);
  assert.deepEqual(apply([], reinsertions(before, before, [])), before);
});

test('undo works after other changes to the playlist', () => {
  // Take out 1, then 2; undo 1, then undo 2.
  let list = [3, 4];
  list = apply(list, reinsertions([1, 2, 3, 4], [1], list));
  assert.deepEqual(list, [1, 3, 4]);
  list = apply(list, reinsertions([2, 3, 4], [2], list));
  assert.deepEqual(list, [1, 2, 3, 4]);
  // The list was reordered meanwhile: the track goes back after its old neighbour.
  assert.deepEqual(apply([4, 1, 3], reinsertions([1, 2, 3, 4], [2], [4, 1, 3])), [4, 1, 2, 3]);
  // A track added back by hand meanwhile stays where it is.
  assert.deepEqual(reinsertions([1, 2, 3], [2, 3], [1, 3]), [{ ids: [2], index: 1 }]);
});
