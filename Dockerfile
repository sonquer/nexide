# syntax=docker/dockerfile:1.7
#
# Multi-stage build for nexide.
#
# Build stage runs on Debian because the `v8` crate (rusty_v8) is built and
# linked against glibc; building it under musl requires either bespoke V8
# patches or an unsupported toolchain. The runtime image is also Debian-slim
# (glibc) so the dynamically-linked binary loads without resorting to Alpine
# `gcompat`, which historically misses libresolv symbols (e.g. `__res_init`)
# pulled in transitively through glibc's NSS / `getaddrinfo` chain.

ARG RUST_VERSION=1.95
ARG DEBIAN_VERSION=bookworm

FROM rust:${RUST_VERSION}-bookworm AS builder
WORKDIR /build

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        clang \
        pkg-config \
        python3 \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

RUN --mount=type=cache,target=/build/target \
    --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    cargo build --release --locked --bin nexide \
    && cp target/release/nexide /usr/local/bin/nexide

FROM debian:${DEBIAN_VERSION}-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        libgcc-s1 \
        libstdc++6 \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system nexide \
    && useradd --system --gid nexide --uid 10001 --no-create-home nexide \
    && groupadd --system --gid 1001 nodejs \
    && useradd --system --gid nodejs --uid 1001 --no-create-home nextjs

COPY --from=builder /usr/local/bin/nexide /usr/local/bin/nexide

USER nexide
WORKDIR /app

ENV HOSTNAME=0.0.0.0 \
    PORT=3000 \
    RUST_LOG=info

EXPOSE 3000

ENTRYPOINT ["/usr/local/bin/nexide"]
CMD ["start", "/app"]
