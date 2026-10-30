import assert from 'node:assert/strict';
import { test } from 'node:test';
import { addedActive, addedChip, addedFilter, addedSelectionOf, addedWords, dayStart, NO_ADDED } from './added.ts';

const local = (y: number, m: number, d: number) => new Date(y, m - 1, d).getTime();
const NOW = local(2026, 10, 29) + 15 * 3_600_000;

test('a period moves with the clock; days are local midnights, the last one included', () => {
  assert.equal(addedActive(NO_ADDED), false);
  assert.deepEqual(addedFilter(NO_ADDED), {});
  const at = new Date(NOW);
  assert.deepEqual(addedFilter({ period: '7d', from: '2026-01-01', to: null }, at), { addedPeriod: '7d', utcOffsetMin: -at.getTimezoneOffset() }, 'a period wins over days');
  assert.deepEqual(addedFilter({ period: null, from: '2026-10-01', to: '2026-10-20' }), { addedAfter: local(2026, 10, 1), addedBefore: local(2026, 10, 21) });
  assert.deepEqual(addedFilter({ period: null, from: null, to: '2026-12-31' }), { addedBefore: local(2027, 1, 1) });
  assert.equal(dayStart('2026-02-30'), null, 'no such day');
  assert.equal(dayStart('yesterday'), null);
  assert.equal(addedActive({ period: null, from: 'nonsense', to: null }), false);
});

test('the filter button and smart playlists say it in words', () => {
  assert.equal(addedChip({ period: '30d', from: null, to: null }, NOW), '30 days');
  assert.equal(addedChip({ period: null, from: '2026-10-01', to: null }, NOW), `since ${addedChip({ period: null, from: '2026-10-01', to: '2026-10-01' }, NOW)}`);
  assert.match(addedChip({ period: null, from: '2026-10-01', to: '2026-10-20' }, NOW) ?? '', / – /);
  assert.equal(addedWords({ addedPeriod: '7d' }, NOW), 'added in the last 7 days');
  assert.equal(addedWords({ addedPeriod: 'today' }, NOW), 'added today');
  assert.equal(addedWords({ addedPeriod: 'year' }, NOW), 'added this year');
  assert.match(addedWords({ addedAfter: local(2026, 10, 1), addedBefore: local(2026, 10, 21) }, NOW) ?? '', /^added .+ to .+/);
  assert.equal(addedWords({}, NOW), null);
});

test('a saved range shows the same days again', () => {
  const sel = { period: null, from: '2026-03-29', to: '2026-10-25' };
  assert.deepEqual(addedSelectionOf(addedFilter(sel)), sel, 'across a change of clocks too');
  assert.deepEqual(addedSelectionOf({ addedPeriod: '90d' }), { period: '90d', from: null, to: null });
});
