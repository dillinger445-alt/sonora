import assert from 'node:assert/strict';
import { test } from 'node:test';
import type { PreviewSession } from '../api/types.ts';
import { enqueue, playableUpTo, previewBlock, previewKey, previewTrackId, previewWords, putFirst, rowWords, takeNext, without, type PreviewSource } from './previews.ts';

const src = (name: string, username = 'night-owl'): PreviewSource => ({
  username,
  filename: `@@shared\\Music\\Afterhours\\${name}`,
  name,
  folder: 'Afterhours',
  size: 1_000_000,
  length: 120,
});

const session = (over: Partial<PreviewSession> = {}): PreviewSession => ({
  id: 1,
  username: 'night-owl',
  filename: 'x.flac',
  name: 'x.flac',
  size: 1_000_000,
  state: 'ready',
  transfer: 'downloading',
  ready: true,
  queue: null,
  received: 500_000,
  decodedSeconds: 30,
  totalSeconds: 120,
  rate: 44_100,
  channels: 2,
  followsDownload: false,
  handedOver: false,
  limitReached: false,
  endsEarly: false,
  error: null,
  ...over,
});

test('a preview has its own negative id, the same for the same file', () => {
  const a = src('01 First Light.flac');
  assert.ok(previewTrackId(a) < 0, 'library tracks have positive ids');
  assert.equal(previewTrackId(a), previewTrackId({ ...a }));
  assert.notEqual(previewTrackId(a), previewTrackId(src('01 First Light.flac', 'someone-else')));
  assert.notEqual(previewTrackId(a), previewTrackId(src('02 Tape Hiss.wav')));
  assert.equal(previewKey(a), `night-owl\n${a.filename}`);
});

test('only FLAC, WAV and AIFF from slskd on this computer can be previewed, and the rest say why', () => {
  for (const ext of ['flac', 'FLAC', 'wav', 'wave', 'aif', 'aiff', 'aifc']) assert.equal(previewBlock({ ext, size: 10 }, true), null, ext);
  assert.match(previewBlock({ ext: 'mp3', size: 10 }, true) ?? '', /^MP3 cannot be previewed\. Previews are for FLAC, WAV and AIFF\.$/);
  assert.match(previewBlock({ ext: 'm4a', size: 10 }, true) ?? '', /at their end/);
  assert.match(previewBlock({ ext: '', size: 10 }, true) ?? '', /^This file cannot be previewed/);
  assert.match(previewBlock({ ext: 'flac', size: 0 }, true) ?? '', /size is unknown/);
  assert.equal(previewBlock({ ext: 'flac', size: 10 }, false), 'Previews need slskd on this computer.');
});

test('the preview queue: add once, play next, take out, go on', () => {
  const a = src('01.flac');
  const b = src('02.wav');
  const c = src('03.aiff');
  let q: PreviewSource[] = [];
  q = enqueue(q, a);
  q = enqueue(q, b);
  const same = enqueue(q, { ...a });
  assert.equal(same, q, 'adding a queued file again changes nothing');
  q = enqueue(q, c);
  assert.deepEqual(q.map((x) => x.name), ['01.flac', '02.wav', '03.aiff']);
  q = putFirst(q, c);
  assert.deepEqual(q.map((x) => x.name), ['03.aiff', '01.flac', '02.wav'], 'play next moves it to the front');
  q = putFirst(q, src('04.flac'));
  assert.deepEqual(q.map((x) => x.name), ['04.flac', '03.aiff', '01.flac', '02.wav'], 'play next adds one that was not queued');
  q = without(q, a);
  assert.deepEqual(q.map((x) => x.name), ['04.flac', '03.aiff', '02.wav']);
  const step = takeNext(q);
  assert.equal(step?.next.name, '04.flac');
  assert.deepEqual(step?.rest.map((x) => x.name), ['03.aiff', '02.wav']);
  assert.equal(takeNext([]), null, 'nothing to go on to');
  // The same file from another user is another preview.
  assert.equal(enqueue([a], src('01.flac', 'other')).length, 2);
});

test('a preview plays and seeks only as far as it has arrived', () => {
  assert.equal(playableUpTo(null), 0);
  assert.equal(playableUpTo(session({ decodedSeconds: null })), 0);
  assert.equal(playableUpTo(session({ decodedSeconds: 30 })), 29.7);
  assert.equal(playableUpTo(session({ decodedSeconds: 0.1 })), 0);
  assert.equal(playableUpTo(session({ transfer: 'complete', decodedSeconds: 30 })), Number.POSITIVE_INFINITY);
});

test('how a preview is going, in words', () => {
  const w = (over: Partial<PreviewSession>) => previewWords(session(over), 'night-owl');
  assert.equal(previewWords(null, 'night-owl'), 'Asking night-owl…');
  assert.equal(w({ state: 'connecting', ready: false }), 'Asking night-owl…');
  assert.equal(w({ state: 'queued', ready: false, queue: 3 }), 'Number 3 in night-owl’s queue');
  assert.equal(w({ state: 'queued', ready: false }), 'Waiting in night-owl’s queue');
  assert.equal(w({ state: 'buffering' }), 'Buffering…');
  assert.equal(w({ state: 'reconnecting' }), 'The connection dropped; asking again…');
  assert.equal(w({ state: 'ready' }), 'Arriving as it plays');
  assert.equal(w({ state: 'ready', transfer: 'paused' }), 'Enough has arrived for now');
  assert.equal(w({ state: 'ready', transfer: 'complete' }), 'All of it has arrived');
  // B105: while the player waits for audio, nothing says enough has arrived.
  assert.equal(previewWords(session({ state: 'ready', transfer: 'paused' }), 'night-owl', true), 'Buffering…');
  assert.equal(previewWords(session({ state: 'ready', transfer: 'complete' }), 'night-owl', true), 'Buffering…');
  assert.equal(previewWords(session({ state: 'queued', ready: false, queue: 2 }), 'night-owl', true), 'Number 2 in night-owl’s queue');
  assert.equal(w({ state: 'unavailable', error: 'This WAV holds MP3 audio, which previews cannot play. Download it to listen.' }), 'This WAV holds MP3 audio, which previews cannot play. Download it to listen.');
  assert.equal(w({ state: 'failed', error: null }), 'This preview cannot be played.');
  assert.equal(w({ limitReached: true }), 'The preview stops here: its cache is full');
  assert.equal(w({ endsEarly: true }), 'The transfer stopped, so the preview ends early');
  assert.equal(w({ handedOver: true }), 'Downloading; it joins your library when it finishes');
  assert.equal(w({ followsDownload: true }), 'Playing along with the download');
});

test('a result row says Playing, Paused or why not', () => {
  const p = session();
  assert.equal(rowWords({ status: 'playing', error: null, preview: p }, 'night-owl'), 'Playing');
  assert.equal(rowWords({ status: 'paused', error: null, preview: p }, 'night-owl'), 'Paused');
  assert.equal(rowWords({ status: 'paused', error: null, preview: session({ ready: false, state: 'connecting' }) }, 'night-owl'), 'Asking night-owl…');
  assert.equal(rowWords({ status: 'loading', error: null, preview: session({ state: 'buffering' }) }, 'night-owl'), 'Buffering…');
  assert.equal(rowWords({ status: 'loading', error: null, preview: session({ state: 'ready', transfer: 'paused' }) }, 'night-owl'), 'Buffering…');
  assert.equal(rowWords({ status: 'error', error: 'night-owl turned the request down.', preview: p }, 'night-owl'), 'night-owl turned the request down.');
  assert.equal(rowWords({ status: 'error', error: null, preview: null }, 'night-owl'), 'Playback failed');
});
