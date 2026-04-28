# syntax=docker/dockerfile:1.7
#
# Node.js bench image - runs the canonical Next.js standalone bundle.
#
# Stage 1: install deps + run `next build --output=standalone`.
# Stage 2: slim runtime containing only the standalone bundle.
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

FROM node:24-bookworm-slim AS runtime
WORKDIR /app
ENV NODE_ENV=production \
    HOSTNAME=0.0.0.0 \
    PORT=3000 \
    NEXT_TELEMETRY_DISABLED=1
COPY --from=next-builder /build/e2e/next-fixture/.next/standalone/  /app/
COPY --from=next-builder /build/e2e/next-fixture/.next/static       /app/.next/static
COPY --from=next-builder /build/e2e/next-fixture/public             /app/public
EXPOSE 3000
CMD ["node", "server.js"]
