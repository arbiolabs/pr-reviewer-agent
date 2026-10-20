# pr-review serve: webhook receiver, worker and reviews in one process. State lives on the /data volume.
FROM node:24-slim

RUN apt-get update \
  && apt-get install -y --no-install-recommends git ca-certificates \
  && rm -rf /var/lib/apt/lists/* \
  && npm install -g pnpm@12.11.2

WORKDIR /app
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
RUN pnpm install --frozen-lockfile --prod
COPY bin ./bin
COPY src ./src
COPY migrations ./migrations
COPY prompts ./prompts

RUN mkdir /data && chown node:node /data
USER node
ENV NODE_ENV=production PR_REVIEW_DATA_DIR=/data PORT=8080
VOLUME /data
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s CMD node -e "fetch('http://127.0.0.1:' + process.env.PORT + '/healthz').then((r) => process.exit(r.ok ? 0 : 1), () => process.exit(1))"
CMD ["node", "bin/pr-review.mjs", "serve"]
