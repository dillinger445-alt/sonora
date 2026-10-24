import assert from 'node:assert/strict';
import { test } from 'node:test';
import { parseStepValue } from './stepper.ts';

test('an empty or unreadable field is no value; numbers are clamped and rounded', () => {
  // B46: Number('') is 0, which used to save the minimum.
  assert.equal(parseStepValue('', 1, 1440), null);
  assert.equal(parseStepValue('   ', 1, 16), null);
  assert.equal(parseStepValue('abc', 1, 16), null);
  assert.equal(parseStepValue('0', 1, 16), 1);
  assert.equal(parseStepValue('99', 1, 16), 16);
  assert.equal(parseStepValue(' 7 ', 1, 16), 7);
  assert.equal(parseStepValue('7.6', 1, 16), 8);
  assert.equal(parseStepValue('23', 0, 100, 5), 25);
});
