import assert from 'node:assert/strict';
import { test } from 'node:test';
import { formatRoute, parseRoute, type Route } from './router.ts';

test('routes round-trip through the hash', () => {
  const routes: Route[] = [
    { view: 'overview' },
    { view: 'library' },
    { view: 'library', folderId: 3 },
    { view: 'queue' },
    { view: 'folders' },
    { view: 'attention' },
    { view: 'attention', flag: 'clipping' },
    { view: 'duplicates' },
    { view: 'history' },
    { view: 'history', scanId: 12 },
    { view: 'settings' },
    { view: 'track', id: 42 },
  ];
  for (const r of routes) assert.deepEqual(parseRoute(formatRoute(r)), r);
});

test('unknown or malformed hashes fall back safely', () => {
  assert.deepEqual(parseRoute(''), { view: 'overview' });
  assert.deepEqual(parseRoute('#/nowhere'), { view: 'overview' });
  assert.deepEqual(parseRoute('#/track/abc'), { view: 'library' });
  assert.deepEqual(parseRoute('#/track/-4'), { view: 'library' });
  assert.deepEqual(parseRoute('#/history/0'), { view: 'history' });
  assert.deepEqual(parseRoute('#/library?folder=x'), { view: 'library' });
});
