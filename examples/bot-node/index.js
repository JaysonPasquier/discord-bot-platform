import { PlatformClient } from '../../sdk/node/src/index.js';

export function parseGuildIds(raw) {
  return (raw ?? '')
    .split(',')
    .map((id) => id.trim())
    .filter(Boolean);
}

export async function checkOnce(client, guildIds, log) {
  for (const id of guildIds) {
    try {
      log(`guild ${id} premium=${await client.isPremium(id)}`);
    } catch (error) {
      log(`guild ${id} error=${error.name}`);
    }
  }
}

// Ce bot n'a pas besoin de Discord : il montre seulement le contrat avec la plateforme.
async function main(env) {
  const client = new PlatformClient({ baseUrl: env.PLATFORM_API_URL, token: env.PLATFORM_API_TOKEN });
  const guildIds = parseGuildIds(env.GUILD_IDS);
  const intervalMs = Number(env.INTERVAL_MS ?? 5000);
  do {
    await checkOnce(client, guildIds, console.log);
    if (env.ONCE) return;
    await new Promise((resolve) => setTimeout(resolve, intervalMs));
  } while (true);
}

if (import.meta.filename === process.argv[1]) await main(process.env);
