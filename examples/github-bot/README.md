# Bot d'exemple pour la plateforme (déploiement depuis GitHub)

Modèle de dépôt : copie ce dossier à la racine d'un nouveau dépôt GitHub, pousse-le, puis connecte-le au bot sur le dashboard.

- `bot.toml` : ce qui est gratuit et ce qui est payant (lu par la plateforme).
- `index.js` : le bot discord.js. Il lit `TIER` (`free` ou `premium`) et interroge l'API de la plateforme avec le SDK (`import ... from "platform-sdk"` : le SDK est ajouté à l'image par la plateforme).
- Commandes : `/ping`, `/premium-status`, `/vip`.

Variables fournies par la plateforme : `DISCORD_TOKEN`, `PLATFORM_API_URL`, `PLATFORM_API_TOKEN`, `TIER`, `DASHBOARD_URL`.
