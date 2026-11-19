import assert from 'node:assert/strict';
import { test } from 'node:test';

import { PlatformAuthError, PlatformClient, PlatformRequestError } from '../src/index.js';

const json = (status, body) =>
  new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json' } });

const ok = (access) => json(200, { guild_id: '1', status: access ? 'active' : 'expired', access });

function setup(responses, options = {}) {
  const calls = [];
  const queue = [...responses];
  const clock = { t: 1_000_000 };
  const fetch = async (url, init) => {
    calls.push({ url, init });
    const next = queue.shift();
    if (next instanceof Error) throw next;
    return next;
  };
  const client = new PlatformClient({
    baseUrl: 'http://api.test',
    token: 'tok',
    fetch,
    now: () => clock.t,
    refreshMs: 60_000,
    cacheTtlMs: 86_400_000,
    ...options,
  });
  return { client, calls, clock };
}

test('asks the right URL with the bearer token and no token in the URL', async () => {
  const { client, calls } = setup([ok(true)]);
  assert.equal(await client.isPremium('123'), true);
  assert.equal(calls.length, 1);
  assert.equal(calls[0].url, 'http://api.test/v1/premium/123');
  assert.equal(calls[0].init.headers.authorization, 'Bearer tok');
  assert.ok(!calls[0].url.includes('tok'));
});

test('returns false when the guild has no access', async () => {
  const { client } = setup([ok(false)]);
  assert.equal(await client.isPremium('1'), false);
});

test('a trailing slash in the base URL is ignored', async () => {
  const { client, calls } = setup([ok(true)], { baseUrl: 'http://api.test///' });
  await client.isPremium('1');
  assert.equal(calls[0].url, 'http://api.test/v1/premium/1');
});

test('serves from the cache until refreshMs has passed', async () => {
  const { client, calls, clock } = setup([ok(true), ok(false)]);
  assert.equal(await client.isPremium('1'), true);
  clock.t += 59_999;
  assert.equal(await client.isPremium('1'), true);
  assert.equal(calls.length, 1);
  clock.t += 1;
  assert.equal(await client.isPremium('1'), false);
  assert.equal(calls.length, 2);
});

test('each guild has its own cache entry', async () => {
  const { client, calls } = setup([ok(true), ok(false)]);
  assert.equal(await client.isPremium('1'), true);
  assert.equal(await client.isPremium('2'), false);
  assert.equal(calls.length, 2);
});

test('a server error falls back to the cached answer within the TTL', async () => {
  const { client, clock } = setup([ok(true), json(503, {})]);
  assert.equal(await client.isPremium('1'), true);
  clock.t += 3_600_000;
  assert.equal(await client.isPremium('1'), true);
});

test('the cached answer is still used exactly at the TTL and dropped after', async () => {
  const at = setup([ok(true), json(500, {})]);
  await at.client.isPremium('1');
  at.clock.t += 86_400_000;
  assert.equal(await at.client.isPremium('1'), true);

  const after = setup([ok(true), json(500, {})]);
  await after.client.isPremium('1');
  after.clock.t += 86_400_001;
  assert.equal(await after.client.isPremium('1'), false);
});

test('a network error behaves like a server error', async () => {
  const { client, clock } = setup([ok(true), new Error('offline')]);
  await client.isPremium('1');
  clock.t += 120_000;
  assert.equal(await client.isPremium('1'), true);
});

test('an outage with nothing cached refuses by default and can be told to allow', async () => {
  const strict = setup([json(503, {})]);
  assert.equal(await strict.client.isPremium('1'), false);
  const lenient = setup([json(503, {})], { defaultOnFailure: true });
  assert.equal(await lenient.client.isPremium('1'), true);
});

test('a malformed answer is treated as an outage', async () => {
  const noField = setup([json(200, { status: 'active' })]);
  assert.equal(await noField.client.isPremium('1'), false);
  const notJson = setup([new Response('<html>', { status: 200 })], { defaultOnFailure: true });
  assert.equal(await notJson.client.isPremium('1'), true);
});

test('a rejected token is an error, even when an old answer is cached', async () => {
  const { client, clock } = setup([ok(true), json(401, {})]);
  await client.isPremium('1');
  clock.t += 120_000;
  await assert.rejects(client.isPremium('1'), PlatformAuthError);
});

test('other client errors are reported, not hidden', async () => {
  const { client } = setup([json(400, {})]);
  await assert.rejects(client.isPremium('1'), PlatformRequestError);
});

test('a fresh answer replaces the cached one', async () => {
  const { client, clock } = setup([ok(true), ok(false)]);
  assert.equal(await client.isPremium('1'), true);
  clock.t += 60_000;
  assert.equal(await client.isPremium('1'), false);
  clock.t += 1_000;
  assert.equal(await client.isPremium('1'), false);
});

test('guild ids must be decimal strings or bigints', async () => {
  const { client, calls } = setup([ok(true), ok(true)]);
  for (const bad of [123, 1.5, '', 'abc', '12 3', '-1', '+1', '1'.repeat(21), -1n, null, undefined, {}]) {
    await assert.rejects(client.isPremium(bad), TypeError, String(bad));
  }
  assert.equal(calls.length, 0);
  await client.isPremium(1234567890123456789n);
  await client.isPremium('1234567890123456789');
  assert.equal(calls[0].url, 'http://api.test/v1/premium/1234567890123456789');
});

test('zero and the largest 64-bit id are accepted', async () => {
  const { client, calls } = setup([ok(true), ok(true)]);
  await client.isPremium('18446744073709551615');
  await client.isPremium(0n);
  assert.equal(calls.length, 2);
});

test('every request carries a timeout signal', async () => {
  const { client, calls } = setup([ok(true)]);
  await client.isPremium('1');
  assert.ok(calls[0].init.signal instanceof AbortSignal);
});

test('baseUrl and token are required', () => {
  assert.throws(() => new PlatformClient({ token: 't' }), TypeError);
  assert.throws(() => new PlatformClient({ baseUrl: 'http://x' }), TypeError);
  assert.throws(() => new PlatformClient(), TypeError);
});
