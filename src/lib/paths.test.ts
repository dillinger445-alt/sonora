import assert from 'node:assert/strict';
import { test } from 'node:test';
import { isAbsolutePath, parsePathList } from './paths.ts';

test('absolute paths on every platform', () => {
  assert.ok(isAbsolutePath('/Users/me/Music'));
  assert.ok(isAbsolutePath('C:\\Music'));
  assert.ok(isAbsolutePath('d:/Music'));
  assert.ok(isAbsolutePath('\\\\nas\\share\\Music'));
  assert.ok(!isAbsolutePath('Music/Techno'));
  assert.ok(!isAbsolutePath('~/Music'));
});

test('path lists: quotes, blank lines, duplicates and invalid entries', () => {
  const r = parsePathList('"/Users/me/Music"\n\n  /Volumes/USB/Sets  \n/Users/me/Music\nrelative/path\r\nC:\\DJ');
  assert.deepEqual(r.paths, ['/Users/me/Music', '/Volumes/USB/Sets', 'C:\\DJ']);
  assert.deepEqual(r.invalid, ['relative/path']);
});
