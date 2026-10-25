import assert from 'node:assert/strict';
import { test } from 'node:test';
import type { PlaylistCandidate, PlaylistItem, PlaylistRun } from '../api/types.ts';
import { canInclude, itemWords, matches, pickOf, quality, runWords, searchWords, trackName } from './playlist.ts';

const candidate = (over: Partial<PlaylistCandidate> = {}): PlaylistCandidate => ({
  username: 'good',
  filename: '@@a\\Music\\01 Rain.flac',
  name: '01 Rain.flac',
  ext: 'flac',
  size: 40_000_000,
  bitDepth: 16,
  sampleRate: 44_100,
  bitRate: null,
  length: 432,
  lossless: true,
  freeSlot: true,
  queue: 0,
  speed: 1_000_000,
  overlap: 1,
  rejected: null,
  ...over,
});

const item = (over: Partial<PlaylistItem> = {}): PlaylistItem => ({
  id: 1,
  runId: 1,
  seq: 0,
  videoId: 'x',
  url: 'https://www.youtube.com/watch?v=x',
  ytTitle: 'Kerri Chandler - Rain (Original Mix)',
  channel: null,
  seconds: 432,
  artist: 'Kerri Chandler',
  title: 'Rain',
  mix: 'Original Mix',
  catalogue: null,
  included: true,
  leftOut: null,
  status: 'pending',
  variation: null,
  responses: null,
  candidates: [],
  chosen: null,
  attempts: 0,
  transfer: null,
  note: null,
  error: null,
  trackId: null,
  track: null,
  updatedAt: 0,
  ...over,
});

const run = (over: Partial<PlaylistRun> = {}): PlaylistRun => ({
  id: 1,
  createdAt: 0,
  updatedAt: 0,
  state: 'running',
  source: 'PLx',
  playlistId: 'PLx',
  title: 'Late Night',
  rules: null,
  confirmed: true,
  passes: 0,
  retryAt: null,
  error: null,
  finishedAt: null,
  counts: { total: 10, included: 8, leftOut: 2, pending: 3, searching: 1, found: 1, notFound: 1, downloading: 1, verifying: 0, done: 1, failed: 0 },
  current: { itemId: 4, title: 'Moodymann – Shades Of Jae', variation: 2 },
  ...over,
});

test('a track is named by artist and title, and its pick is the chosen candidate', () => {
  assert.equal(trackName(item()), 'Kerri Chandler – Rain');
  assert.equal(trackName(item({ artist: null })), 'Rain');
  const a = candidate();
  const b = candidate({ username: 'other' });
  assert.equal(pickOf(item({ candidates: [a, b], chosen: 1 })), b);
  assert.equal(pickOf(item({ candidates: [a, b] })), a, 'the first when none was chosen');
  assert.equal(pickOf(item()), null);
  // B53: a not-found track's best file was turned down by the rules: no pick.
  assert.equal(pickOf(item({ status: 'not_found', candidates: [candidate({ rejected: 'lossy formats are off' })] })), null);
  assert.equal(quality(candidate()), '16-bit / 44.1 kHz');
  assert.equal(quality(candidate({ bitDepth: null, sampleRate: null })), 'resolution not reported');
  assert.equal(quality(candidate({ lossless: false, bitRate: 320 })), '320 kbps');
});

test('how a track is going, in words', () => {
  const w = (over: Partial<PlaylistItem>, reviewing = false) => itemWords(item(over), reviewing);
  assert.deepEqual(w({ status: 'pending' }), { text: 'Waiting to search', tone: 'faint' });
  assert.deepEqual(w({ status: 'searching', variation: 3 }), { text: 'Searching (try 3)', tone: '' });
  assert.deepEqual(w({ status: 'found' }), { text: 'Found, waiting to download', tone: 't-lime' });
  assert.deepEqual(w({ status: 'found' }, true), { text: 'Pick', tone: 't-lime' });
  assert.deepEqual(w({ status: 'not_found', error: 'No one answered with this track.' }), { text: 'No one answered with this track.', tone: 't-amber' });
  assert.deepEqual(w({ status: 'downloading', transfer: { id: 't', username: 'good', filename: 'f', size: 1000, bytes: 430 } }), { text: 'Downloading 43%', tone: '' });
  assert.deepEqual(w({ status: 'downloading', transfer: { id: 't', username: 'good', filename: 'f', size: 1000, bytes: 0 } }), { text: 'Asked for; waiting', tone: '' });
  assert.deepEqual(w({ status: 'verifying' }), { text: 'Arrived; checking it', tone: '' });
  assert.deepEqual(w({ status: 'done' }), { text: 'Done', tone: 't-lime' });
  assert.deepEqual(w({ status: 'done', track: { overall: 91.6 } as PlaylistItem['track'] }), { text: 'Done, scored 92', tone: 't-lime' });
  assert.deepEqual(w({ status: 'failed', error: 'Gave up after 3 tries' }), { text: 'Gave up after 3 tries', tone: 't-red' });
  assert.deepEqual(w({ included: false, leftOut: 'A Short, not a track.' }), { text: 'A Short, not a track.', tone: 'faint' });
});

test('how the run is going, in words', () => {
  assert.equal(runWords(run()), 'Searching for Moodymann – Shades Of Jae (try 2)');
  assert.equal(runWords(run({ current: null })), 'Downloading');
  assert.equal(runWords(run({ current: null, counts: { ...run().counts, downloading: 0 } })), 'Running');
  assert.match(runWords(run({ state: 'paused' })), /^Paused/);
  assert.equal(runWords(run({ state: 'review' })), 'Searched. 1 pick ready to download; 1 track not found.');
  assert.equal(runWords(run({ state: 'finished', counts: { ...run().counts, done: 6, failed: 1, notFound: 1 } })), 'Done: 6 tracks downloaded and checked, 1 failed, 1 not found.');
  assert.equal(runWords(run({ state: 'finished', counts: { ...run().counts, done: 8, failed: 0, notFound: 0 } })), 'Done: 8 tracks downloaded and checked.');
  assert.equal(runWords(run({ state: 'cancelled' })), 'Stopped.');
  assert.equal(runWords(run({ state: 'failed', error: 'YouTube could not be reached' })), 'YouTube could not be reached');
  // An hour from now: a time, or a date and a time once that is tomorrow (it failed in CI just before midnight).
  assert.match(runWords(run({ state: 'waiting', retryAt: Date.now() + 3_600_000 })), /looking again at (\d|[A-Z][a-z]{2} \d+, \d)/);
});

test('filters pick the rows they say', () => {
  const rows = [
    item({ id: 1, status: 'found' }),
    item({ id: 2, status: 'downloading' }),
    item({ id: 3, status: 'verifying' }),
    item({ id: 4, status: 'done' }),
    item({ id: 5, status: 'not_found' }),
    item({ id: 6, status: 'failed' }),
    item({ id: 7, included: false, leftOut: 'A Short, not a track.' }),
  ];
  const ids = (f: Parameters<typeof matches>[1]) => rows.filter((r) => matches(r, f)).map((r) => r.id);
  assert.deepEqual(ids('all'), [1, 2, 3, 4, 5, 6, 7]);
  assert.deepEqual(ids('found'), [1]);
  assert.deepEqual(ids('downloading'), [2, 3], 'arrived files being checked count as downloading');
  assert.deepEqual(ids('done'), [4]);
  assert.deepEqual(ids('not_found'), [5]);
  assert.deepEqual(ids('failed'), [6]);
  assert.deepEqual(ids('left_out'), [7]);
});

test('a left-out video can be put back only while the run can still look for it', () => {
  // B17: ticked in a finished run, it waited for ever.
  const left = item({ included: false, leftOut: 'A Short, not a track.', status: 'pending' });
  assert.equal(canInclude(left, 'draft'), true);
  assert.equal(canInclude(left, 'running'), true);
  assert.equal(canInclude(left, 'paused'), true);
  assert.equal(canInclude(left, 'finished'), false);
  assert.equal(canInclude(left, 'cancelled'), false);
  assert.equal(canInclude(item({ status: 'not_found' }), 'finished'), true, 'Retry looks for it again');
  assert.equal(canInclude(item({ status: 'done' }), 'running'), false);
});

test('each search is said in words', () => {
  assert.equal(searchWords(1), 'the title');
  assert.equal(searchWords(6), 'the artist alone');
  assert.equal(searchWords(10), 'the artist and the title, simply spelt');
  assert.equal(searchWords(11), 'search 11');
});
