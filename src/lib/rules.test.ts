import assert from 'node:assert/strict';
import { test } from 'node:test';
import type { SmartRules } from '../api/types.ts';
import { describeRules, organizeLeftOut } from './rules.ts';

const rules = (filter: SmartRules['filter']): SmartRules => ({ filter, sort: { key: 'bpm', desc: false } });

test('every rule is described, with the words the rules dialog uses', () => {
  // B44: these used to read "All tracks", "score 35+" or nothing at all.
  assert.equal(describeRules(rules({ flags: ['clipping'] }), 'camelot'), 'issue clipping; by tempo');
  assert.equal(describeRules(rules({ attention: true }), 'camelot'), 'needs attention; by tempo');
  assert.equal(describeRules(rules({ scoreKind: 'overall', minScore: 35, maxScore: 54.999 }), 'camelot'), 'overall score 35–54; by tempo');
  assert.equal(describeRules(rules({ scoreKind: 'listening', maxScore: 54.999 }), 'camelot'), 'listening score up to 54; by tempo');
  assert.equal(describeRules(rules({ scoreKind: 'dj', minScore: 75 }), 'camelot'), 'DJ score 75+; by tempo');
  assert.equal(describeRules(rules({ statuses: ['failed'], folderId: 3 }), 'camelot'), 'status failed; one library folder; by tempo');
  assert.equal(describeRules(rules({ bpmMin: 120, bpmMax: 125.999, keys: [21] }), 'camelot'), '120–125 BPM; 8A; by tempo');
  assert.equal(describeRules(rules({}), 'camelot'), 'All tracks; by tempo');
});

test('sorting into playlists says what it left out', () => {
  // B45: only tracks without a genre were mentioned.
  assert.equal(organizeLeftOut({ without: 0, leftOut: 0, missing: 0 }, 'genre'), undefined);
  assert.equal(organizeLeftOut({ without: 2, leftOut: 0, missing: 0 }, 'key'), '2 tracks without a key stayed out.');
  assert.equal(organizeLeftOut({ without: 0, leftOut: 3, missing: 1 }, 'genre'), '3 more genres got no playlist: one folder holds at most 200. 1 track whose file is missing stayed out.');
});
