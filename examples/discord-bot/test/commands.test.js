import assert from 'node:assert/strict';
import { test } from 'node:test';

import { PlatformAuthError } from '../../../sdk/node/src/index.js';
import { COMMANDS, handleCommand } from '../src/commands.js';

function platform(answer) {
  const calls = [];
  return {
    calls,
    isPremium: async (guildId) => {
      calls.push(guildId);
      if (answer instanceof Error) throw answer;
      return answer;
    },
  };
}

const ctx = (name, tier = 'premium', extra = {}) => ({ name, guildId: '10', tier, dashboardUrl: 'http://dash', ...extra });

test('ping answers with the tier and does not call the platform', async () => {
  const p = platform(true);
  assert.equal(await handleCommand(ctx('ping', 'free'), p), 'Pong! This is the free bot.');
  assert.equal(await handleCommand(ctx('ping', 'premium'), p), 'Pong! This is the premium bot.');
  assert.deepEqual(p.calls, []);
});

test('commands only work inside a server', async () => {
  const p = platform(true);
  assert.equal(await handleCommand(ctx('ping', 'free', { guildId: null }), p), 'Use this command inside a server.');
  assert.equal(await handleCommand(ctx('vip', 'premium', { guildId: undefined }), p), 'Use this command inside a server.');
  assert.deepEqual(p.calls, []);
});

test('premium-status reports the platform answer for this server', async () => {
  const yes = platform(true);
  assert.equal(await handleCommand(ctx('premium-status'), yes), 'This server has an active premium subscription.');
  assert.deepEqual(yes.calls, ['10']);
  assert.equal(await handleCommand(ctx('premium-status'), platform(false)), 'This server has no active premium subscription.');
});

test('the free bot never unlocks vip and sends people to the market', async () => {
  const p = platform(true);
  const reply = await handleCommand(ctx('vip', 'free'), p);
  assert.equal(reply, 'This is the free bot. Add the premium bot to your server and subscribe: http://dash/market');
  assert.deepEqual(p.calls, []);
});

test('the premium bot unlocks vip only for subscribed servers', async () => {
  assert.equal(await handleCommand(ctx('vip'), platform(true)), 'VIP feature unlocked: this server is premium.');
  assert.equal(await handleCommand(ctx('vip'), platform(false)), 'This server has no active subscription. Subscribe: http://dash/market');
});

test('without a dashboard URL the hint stays generic', async () => {
  const reply = await handleCommand(ctx('vip', 'premium', { dashboardUrl: undefined }), platform(false));
  assert.equal(reply, 'This server has no active subscription. Subscribe: the platform dashboard');
});

test('a refused token and an outage give different messages', async () => {
  const refused = await handleCommand(ctx('premium-status'), platform(new PlatformAuthError('no')));
  assert.equal(refused, 'The bot cannot use the platform: its access token was refused.');
  const down = await handleCommand(ctx('vip'), platform(new Error('boom')));
  assert.equal(down, 'The platform is not reachable right now. Try again later.');
});

test('unknown commands are named in the answer', async () => {
  assert.equal(await handleCommand(ctx('nope'), platform(true)), 'Unknown command: nope.');
});

test('the command list respects the basic Discord limits', () => {
  const names = COMMANDS.map((c) => c.name);
  assert.equal(new Set(names).size, names.length);
  for (const c of COMMANDS) {
    assert.match(c.name, /^[a-z0-9_-]{1,32}$/);
    assert.ok(c.description.length >= 1 && c.description.length <= 100);
  }
  assert.deepEqual(names, ['ping', 'premium-status', 'vip']);
});
