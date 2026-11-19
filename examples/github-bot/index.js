import { Client, Events, GatewayIntentBits, MessageFlags } from 'discord.js';

import { PlatformClient } from 'platform-sdk';
import { COMMANDS, handleCommand } from './src/commands.js';

const { DISCORD_TOKEN, PLATFORM_API_URL, PLATFORM_API_TOKEN, DASHBOARD_URL } = process.env;
if (!DISCORD_TOKEN || !PLATFORM_API_URL || !PLATFORM_API_TOKEN) {
  console.error('DISCORD_TOKEN, PLATFORM_API_URL and PLATFORM_API_TOKEN are required.');
  process.exit(1);
}
const tier = process.env.TIER === 'premium' ? 'premium' : 'free';
const platform = new PlatformClient({ baseUrl: PLATFORM_API_URL, token: PLATFORM_API_TOKEN });
const client = new Client({ intents: [GatewayIntentBits.Guilds] });

// Les commandes de serveur sont disponibles tout de suite, contrairement aux commandes globales.
async function register(guild) {
  try {
    await guild.commands.set(COMMANDS);
    console.log(`commands registered in guild ${guild.id}`);
  } catch (error) {
    console.error(`cannot register commands in guild ${guild.id}: ${error.message}`);
  }
}

client.once(Events.ClientReady, async (ready) => {
  console.log(`logged in as ${ready.user.tag} (${tier} bot) in ${ready.guilds.cache.size} server(s)`);
  for (const guild of ready.guilds.cache.values()) await register(guild);
});
client.on(Events.GuildCreate, register);
client.on(Events.InteractionCreate, async (interaction) => {
  if (!interaction.isChatInputCommand()) return;
  const content = await handleCommand(
    { name: interaction.commandName, guildId: interaction.guildId, tier, dashboardUrl: DASHBOARD_URL },
    platform,
  );
  await interaction.reply({ content, flags: MessageFlags.Ephemeral });
});

await client.login(DISCORD_TOKEN);
