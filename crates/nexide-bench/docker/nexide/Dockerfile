# syntax=docker/dockerfile:1.7
#
# nexide bench image - fully self-contained.
#
# Stage 1: compile `nexide` for the container architecture.
# Stage 2: install npm deps and run `next build --output=standalone`
#          against the next-fixture app, producing the standalone bundle.
# Stage 3: slim runtime that contains only what is needed to serve.
#
# Build context: workspace root (contains `Cargo.toml`, `crates/`,
# and `e2e/next-fixture/`).

FROM rust:1.94-bookworm AS rust-builder
WORKDIR /build
ENV CARGO_TERM_COLOR=never \
    CARGO_NET_RETRY=10 \
    CARGO_INCREMENTAL=0
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --release -p nexide && \
    strip target/release/nexide

FROM node:24-bookworm-slim AS next-builder
WORKDIR /build/next-fixture
ENV NEXT_TELEMETRY_DISABLED=1 \
    CI=1
COPY e2e/next-fixture/package.json e2e/next-fixture/package-lock.json ./
RUN npm install --no-audit --no-fund
COPY e2e/next-fixture/ ./
RUN npx next build

FROM node:24-bookworm-slim AS runtime
WORKDIR /app/next-fixture
ENV NEXT_TELEMETRY_DISABLED=1
COPY e2e/next-fixture/package.json e2e/next-fixture/package-lock.json ./
RUN npm install --omit=dev --no-audit --no-fund && npm cache clean --force
COPY --from=rust-builder   /build/target/release/nexide      /usr/local/bin/nexide
COPY --from=next-builder   /build/e2e/next-fixture/public             /app/e2e/next-fixture/public
COPY --from=next-builder   /build/e2e/next-fixture/.next/static       /app/e2e/next-fixture/.next/static
COPY --from=next-builder   /build/e2e/next-fixture/.next/standalone   /app/e2e/next-fixture/.next/standalone
WORKDIR /app
ENV HOSTNAME=0.0.0.0 \
    PORT=3000 \
    RUST_LOG=info
EXPOSE 3000
ENTRYPOINT ["/usr/local/bin/nexide", "start", "/app/next-fixture"]
