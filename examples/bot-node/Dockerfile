FROM node:24-alpine
WORKDIR /app
COPY sdk/node/package.json sdk/node/package.json
COPY sdk/node/src sdk/node/src
COPY examples/bot-node/package.json examples/bot-node/package.json
COPY examples/bot-node/index.js examples/bot-node/index.js
USER node
CMD ["node", "/app/examples/bot-node/index.js"]
