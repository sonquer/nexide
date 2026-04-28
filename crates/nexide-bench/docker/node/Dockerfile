# syntax=docker/dockerfile:1.7
#
# Node.js bench image — runs the canonical Next.js standalone bundle.
#
# Stage 1: install deps + run `next build --output=standalone`.
# Stage 2: slim runtime containing only the standalone bundle.
#
# Build context: workspace root (only `example/` is needed).

FROM node:20-bookworm-slim AS next-builder
WORKDIR /build/example
ENV NEXT_TELEMETRY_DISABLED=1 \
    CI=1
COPY example/package.json example/package-lock.json ./
RUN npm install --no-audit --no-fund
COPY example/ ./
RUN npx next build

FROM node:20-bookworm-slim AS runtime
WORKDIR /app
ENV NODE_ENV=production \
    HOSTNAME=0.0.0.0 \
    PORT=3000 \
    NEXT_TELEMETRY_DISABLED=1
COPY --from=next-builder /build/example/.next/standalone/  /app/
COPY --from=next-builder /build/example/.next/static       /app/.next/static
COPY --from=next-builder /build/example/public             /app/public
EXPOSE 3000
CMD ["node", "server.js"]
