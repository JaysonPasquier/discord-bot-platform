import assert from 'node:assert/strict';
import { test } from 'node:test';

import { checkOnce, parseGuildIds } from '../index.js';

test('guild ids are split on commas and trimmed', () => {
  assert.deepEqual(parseGuildIds('1, 2 ,3'), ['1', '2', '3']);
  assert.deepEqual(parseGuildIds('1,,2,'), ['1', '2']);
  assert.deepEqual(parseGuildIds(''), []);
  assert.deepEqual(parseGuildIds(undefined), []);
});

test('each guild is reported with its premium status', async () => {
  const client = { isPremium: async (id) => id === '1' };
  const lines = [];
  await checkOnce(client, ['1', '2'], (line) => lines.push(line));
  assert.deepEqual(lines, ['guild 1 premium=true', 'guild 2 premium=false']);
});

test('an error on one guild does not stop the others', async () => {
  const client = {
    isPremium: async (id) => {
      if (id === '1') throw new RangeError('boom');
      return true;
    },
  };
  const lines = [];
  await checkOnce(client, ['1', '2'], (line) => lines.push(line));
  assert.deepEqual(lines, ['guild 1 error=RangeError', 'guild 2 premium=true']);
});
