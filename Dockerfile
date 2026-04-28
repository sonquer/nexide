# syntax=docker/dockerfile:1.7
#
# Multi-stage build for nexide.
#
# Build stage runs on Debian because the `v8` crate (rusty_v8) is built and
# linked against glibc; building it under musl requires either bespoke V8
# patches or an unsupported toolchain. The runtime image stays Alpine for
# size, and uses `gcompat` to run the glibc-linked binary.

ARG RUST_VERSION=1.95
ARG ALPINE_VERSION=3.20

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

FROM alpine:${ALPINE_VERSION} AS runtime

RUN apk add --no-cache \
        ca-certificates \
        gcompat \
        libgcc \
        libstdc++ \
    && addgroup -S nexide \
    && adduser -S -G nexide -u 10001 nexide

COPY --from=builder /usr/local/bin/nexide /usr/local/bin/nexide

USER nexide
WORKDIR /app

ENV NEXIDE_BIND=0.0.0.0:3000 \
    RUST_LOG=info

EXPOSE 3000

ENTRYPOINT ["/usr/local/bin/nexide"]
CMD ["start", "/app"]
