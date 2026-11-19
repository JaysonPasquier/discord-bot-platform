FROM node:24-alpine
WORKDIR /app
COPY sdk/node/package.json sdk/node/package.json
COPY sdk/node/src sdk/node/src
COPY examples/discord-bot/package.json examples/discord-bot/package-lock.json examples/discord-bot/
RUN cd examples/discord-bot && npm ci --omit=dev --ignore-scripts --no-audit --no-fund
COPY examples/discord-bot/index.js examples/discord-bot/index.js
COPY examples/discord-bot/src examples/discord-bot/src
USER node
CMD ["node", "/app/examples/discord-bot/index.js"]
