# syntax=docker/dockerfile:1.7

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
        tini \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 1000 nexide \
    && useradd --system --gid nexide --uid 1000 --no-create-home nexide

COPY --from=builder /usr/local/bin/nexide /usr/local/bin/nexide

USER nexide
WORKDIR /app

ENV HOSTNAME=0.0.0.0 \
    PORT=3000 \
    RUST_LOG=info

EXPOSE 3000

ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/nexide"]
CMD ["start"]