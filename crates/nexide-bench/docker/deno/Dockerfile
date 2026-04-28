# syntax=docker/dockerfile:1.7
#
# Deno bench image - runs the canonical Next.js standalone bundle on
# Deno's Node-compatibility runtime so the comparison is apples-to-
# apples with `nexide` and `node`.
#
# Stage 1: build the same standalone bundle the node and nexide images
#          consume. Built on `node:24-bookworm-slim` because `next
#          build` itself still expects Node tooling.
# Stage 2: slim Deno runtime that only carries the standalone bundle
#          and serves it via `deno run`.
#
# Build context: workspace root (only `e2e/next-fixture/` is needed).

FROM node:24-bookworm-slim AS next-builder
WORKDIR /build/next-fixture
ENV NEXT_TELEMETRY_DISABLED=1 \
    CI=1
COPY e2e/next-fixture/package.json e2e/next-fixture/package-lock.json ./
RUN npm install --no-audit --no-fund
COPY e2e/next-fixture/ ./
RUN npx next build

FROM denoland/deno:2.4.5 AS runtime
WORKDIR /app
ENV NODE_ENV=production \
    HOSTNAME=0.0.0.0 \
    PORT=3000 \
    NEXT_TELEMETRY_DISABLED=1 \
    DENO_NO_UPDATE_CHECK=1
COPY --from=next-builder /build/e2e/next-fixture/.next/standalone/  /app/
COPY --from=next-builder /build/e2e/next-fixture/.next/static       /app/.next/static
COPY --from=next-builder /build/e2e/next-fixture/public             /app/public
EXPOSE 3000
# Deno 2 detects CJS automatically but the Next.js standalone bundle
# mixes CJS `require` with bare-builtin imports (`require('node:fs')`).
# `--unstable-detect-cjs` and `--unstable-bare-node-builtins` are no-ops
# on 2.4+ but kept explicit so the comparison is reproducible across
# minor Deno bumps.
CMD ["deno", "run", \
     "--allow-all", \
     "--unstable-detect-cjs", \
     "--unstable-bare-node-builtins", \
     "--unstable-sloppy-imports", \
     "--quiet", \
     "server.js"]
