import { PlatformAuthError } from '../../../sdk/node/src/index.js';

export const COMMANDS = [
  { name: 'ping', description: 'Check that the bot is alive' },
  { name: 'premium-status', description: 'Show whether this server has premium' },
  { name: 'vip', description: 'A feature reserved for premium servers' },
];

function marketHint(dashboardUrl) {
  return dashboardUrl ? `${dashboardUrl}/market` : 'the platform dashboard';
}

async function vip({ guildId, tier, dashboardUrl }, platform) {
  if (tier !== 'premium') {
    return `This is the free bot. Add the premium bot to your server and subscribe: ${marketHint(dashboardUrl)}`;
  }
  if (await platform.isPremium(guildId)) return 'VIP feature unlocked: this server is premium.';
  return `This server has no active subscription. Subscribe: ${marketHint(dashboardUrl)}`;
}

// La logique est séparée de discord.js pour pouvoir être testée sans connexion à Discord.
export async function handleCommand(context, platform) {
  const { name, guildId, tier } = context;
  if (!guildId) return 'Use this command inside a server.';
  try {
    switch (name) {
      case 'ping':
        return `Pong! This is the ${tier} bot.`;
      case 'premium-status':
        return (await platform.isPremium(guildId))
          ? 'This server has an active premium subscription.'
          : 'This server has no active premium subscription.';
      case 'vip':
        return await vip(context, platform);
      default:
        return `Unknown command: ${name}.`;
    }
  } catch (error) {
    if (error instanceof PlatformAuthError) {
      return 'The bot cannot use the platform: its access token was refused.';
    }
    return 'The platform is not reachable right now. Try again later.';
  }
}
