import assert from 'node:assert/strict';
import { test } from 'node:test';
import { parseNotes } from './notes.ts';

test('release notes: headings, lists with wrapped items, paragraphs', () => {
  const md = '### New\n\n- **Tempo and key** for every\n  analyzed track.\n- Filters\n\nA paragraph\nwrapped.\n\n1. One\n2) Two\n---\nEnd';
  assert.deepEqual(parseNotes(md), [
    { kind: 'h', text: 'New' },
    { kind: 'ul', items: ['**Tempo and key** for every analyzed track.', 'Filters'] },
    { kind: 'p', text: 'A paragraph wrapped.' },
    { kind: 'ol', items: ['One', 'Two'] },
    { kind: 'p', text: 'End' },
  ]);
});

test('release notes: a lazy continuation line stays in its item', () => {
  assert.deepEqual(parseNotes('- one\ntwo\n\nthree'), [
    { kind: 'ul', items: ['one two'] },
    { kind: 'p', text: 'three' },
  ]);
});
