# syntax=docker/dockerfile:1.7
#
# nexide bench image - fully self-contained.
#
# Stage 1: compile `nexide` for the container architecture.
# Stage 2: install npm deps and run `next build --output=standalone`
#          against the example app, producing the standalone bundle.
# Stage 3: slim runtime that contains only what is needed to serve.
#
# Build context: workspace root (contains `Cargo.toml`, `crates/`,
# and `example/`).

FROM rust:1.94-bookworm AS rust-builder
WORKDIR /build
ENV CARGO_TERM_COLOR=never \
    CARGO_NET_RETRY=10 \
    CARGO_INCREMENTAL=0
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --release -p nexide && \
    strip target/release/nexide

FROM node:20-bookworm-slim AS next-builder
WORKDIR /build/example
ENV NEXT_TELEMETRY_DISABLED=1 \
    CI=1
COPY example/package.json example/package-lock.json ./
RUN npm install --no-audit --no-fund
COPY example/ ./
RUN npx next build

FROM node:20-bookworm-slim AS runtime
WORKDIR /app/example
ENV NEXT_TELEMETRY_DISABLED=1
COPY example/package.json example/package-lock.json ./
RUN npm install --omit=dev --no-audit --no-fund && npm cache clean --force
COPY --from=rust-builder   /build/target/release/nexide      /usr/local/bin/nexide
COPY --from=next-builder   /build/example/public             /app/example/public
COPY --from=next-builder   /build/example/.next/static       /app/example/.next/static
COPY --from=next-builder   /build/example/.next/standalone   /app/example/.next/standalone
WORKDIR /app
ENV HOSTNAME=0.0.0.0 \
    PORT=3000 \
    RUST_LOG=info
EXPOSE 3000
ENTRYPOINT ["/usr/local/bin/nexide", "start", "/app/example"]
