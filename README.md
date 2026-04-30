<p align="center">
  <img src="./img/nexide.png" alt="Nexide logo" width="420" />
</p>

[![CI](https://img.shields.io/github/actions/workflow/status/sonquer/nexide/ci.yml?branch=main&label=CI)](https://github.com/sonquer/nexide/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/actions/workflow/status/sonquer/nexide/release.yml?label=release)](https://github.com/sonquer/nexide/actions/workflows/release.yml)
[![GHCR](https://img.shields.io/badge/ghcr.io-sonquer%2Fnexide-blue?logo=docker)](https://github.com/sonquer/nexide/pkgs/container/nexide)
[![Tests](https://img.shields.io/badge/tests-266%20passing-success)](#building-and-testing)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![Rust](https://img.shields.io/badge/rust-2024%20edition-orange.svg)](https://www.rust-lang.org)
[![V8](https://img.shields.io/badge/V8-v147-yellow.svg)](https://v8.dev)
[![Status](https://img.shields.io/badge/status-experimental-red.svg)](#status)

> A native Rust runtime for serving Next.js applications. No Node.js. No Deno.

Nexide embeds a pool of raw V8 isolates behind an Axum/Tower HTTP server, and
exposes just enough of the Node.js compatibility surface for the Next.js
standalone server bundle to boot and serve traffic. It is **not** a general
purpose JavaScript runtime; it is a single-purpose Next.js host.

---

## Status

**Experimental.** The runtime boots a real Next.js 16 standalone bundle, serves
SSR, RSC, ISR, SSG and route handlers correctly, passes 266 unit + integration
tests, and runs continuously under load. It has **not** been hardened for
production traffic and the public Rust API is unstable.

Use it for benchmarking, research, internal tooling, and giving feedback. Do
not put your customers on it yet.

## Why

Node.js is the de facto Next.js host but pays a real tax on cold start, memory
footprint and per-request overhead. Deno improves on parts of that but is not
optimised for the Next.js critical path. Nexide asks a narrower question:

> If we own the entire stack (HTTP, isolate lifecycle, Node API surface),
> how fast can a Next.js standalone bundle actually serve?

## Benchmarks

All numbers below come from `nexide-bench docker-suite`: 5 routes × 3 runtimes
× 4 container presets × 30 s window @ 64 connections, on identical Docker
images of the same Next.js 16 application.

Runtimes: `nexide` (this repo, the binary name), `node` (Node 22 LTS standalone
server), `deno` (Deno 2.x with `--unstable-node-modules-dir`).

### 1 vCPU, 512 MB RAM (typical small container)

| Route          | Runtime | RPS    | p50    | p95    | p99    | Mem avg | CPU avg |
|----------------|---------|-------:|-------:|-------:|-------:|--------:|--------:|
| `api/ping`     | nexide  |  4 664 |  9.9ms | 39.7ms | 52.0ms |  171 MB |   97.9% |
| `api/ping`     | node    |  2 562 | 18.9ms | 47.7ms | 59.7ms |   60 MB |   97.9% |
| `api/ping`     | deno    |  2 994 | 14.1ms | 54.4ms | 61.2ms |  103 MB |   97.3% |
| `api/time`     | nexide  |  4 658 | 10.0ms | 40.4ms | 54.4ms |  146 MB |   97.8% |
| `api/time`     | node    |  2 531 | 19.0ms | 47.7ms | 64.8ms |   60 MB |   97.8% |
| `ssg/about`    | nexide  | 14 130 |  4.3ms |  6.2ms |  7.2ms |  174 MB |   92.3% |
| `ssg/about`    | node    |  3 343 | 15.3ms | 41.1ms | 51.1ms |   59 MB |   98.4% |
| `rsc/about`    | nexide  | 20 846 |  2.7ms |  4.8ms |  9.0ms |  173 MB |  104.1% |
| `rsc/about`    | node    |  3 360 | 15.0ms | 41.4ms | 53.7ms |   59 MB |   98.4% |
| `_next/image`  | nexide  | 13 159 |  4.8ms |  6.5ms |  7.5ms |  175 MB |   29.2% |
| `_next/image`  | node    |  5 934 |  6.5ms | 41.6ms | 52.3ms |   69 MB |   97.6% |
| `_next/image`  | deno    |  5 568 |  7.3ms | 41.3ms | 51.8ms |  126 MB |   97.9% |

The `_next/image` row is the headline: same image, same query string, same
`Accept: image/webp` — Nexide answers it from a native Rust pipeline (decode
→ resize → encode → in-memory LRU + on-disk cache) and never enters V8. CPU
sits at ~30% under the same load that pegs Node and Deno at 100%.

### RPS uplift vs Node.js across all four container presets

| Preset           | api-ping | api-time |  ssg     |  rsc     | _next/image |
|------------------|---------:|---------:|---------:|---------:|------------:|
| 1 vCPU /  256 MB |   +71.1% |   +64.9% |  +303.1% |  +478.1% |     +110.9% |
| 1 vCPU /  512 MB |   +82.1% |   +84.1% |  +322.7% |  +520.4% |     +121.8% |
| 1 vCPU / 1024 MB |   +58.1% |   +76.5% |  +302.7% |  +490.6% |      +52.9% |
| 2 vCPU / 1024 MB |  +145.2% |  +130.5% |  +230.7% |  +394.3% |       +6.0% |

### RPS uplift vs Deno across all four container presets

| Preset           | api-ping | api-time |  ssg     |  rsc     | _next/image |
|------------------|---------:|---------:|---------:|---------:|------------:|
| 1 vCPU /  256 MB |   +49.8% |   +46.9% |  +261.3% |  +411.9% |     +112.7% |
| 1 vCPU /  512 MB |   +55.8% |   +65.6% |  +279.5% |  +458.5% |     +136.3% |
| 1 vCPU / 1024 MB |    +8.0% |   +17.9% |  +206.8% |  +280.4% |      +61.4% |
| 2 vCPU / 1024 MB |   +44.3% |   +33.8% |  +140.6% |  +206.9% |      +11.0% |

Deno closes the gap noticeably on dynamic API routes once it has 1 GB of RAM
to play with (its V8 instance gets room for code-cache + JIT tiering), but
prerendered SSG and RSC routes stay 2-4x ahead because that path bypasses
JavaScript entirely on Nexide.

### What this means honestly

- **Static / RSC routes are 3-5x faster.** This is where the static prerender
  hot path bypasses the V8 pool entirely.
- **Route handlers are ~60-80% faster on a single core, ~140% faster on two.**
  V8 dispatch overhead is the dominant cost; Rust does not magically make your
  JS faster, it makes the path *to* your JS shorter.
- **`/_next/image` is 2x faster at one third of the CPU.** Image optimization
  in Node/Deno goes through the JS `image-optimizer` + `sharp`; in Nexide it
  is a fully native Rust pipeline (`image` + `webp` + `imagequant`) with a
  bounded in-memory LRU in front of the on-disk cache. CPU stays low even
  under saturation, leaving headroom for the rest of the app. The advantage
  shrinks at 2 vCPU because the JS path can finally use the second core.
- **Memory cost is real.** Nexide uses 2-4x the memory of Node.js because it
  pre-warms a pool of V8 isolates. This is the central trade-off.
- **CPU utilisation is broadly comparable on JS routes.** No magic; the wins
  come from parallelism in the pool and a leaner request path, not from
  running V8 less.

Reproduce locally:

```bash
cargo run --release -p nexide-bench -- docker-suite \
    --routes api-ping,api-time,ssg-about,rsc-about,next-image \
    --runtimes nexide,node,deno \
    --presets 1cpu-512mb,2cpu-1024mb \
    --duration 30s --connections 64
```

## Quick start

Requirements: Rust 1.85+ (for edition 2024), a built Next.js standalone bundle.

```bash
# build the runtime
cargo build --release

# build the next-fixture e2e app
pnpm --dir e2e/next-fixture install
pnpm --dir e2e/next-fixture build

# serve it
./target/release/nexide start e2e/next-fixture --port 3000

# probe it
curl -s http://127.0.0.1:3000/api/ping
# {"ok":true,"runtime":"nexide","method":"GET"}
```

## Architecture

```
                           ┌──────────────────────────────┐
   incoming request  ──►   │  Axum / Tower HTTP server    │
                           │  (crates/nexide/src/server)  │
                           └──────────────┬───────────────┘
                                          │
        ┌─────────────────────────────────┼─────────────────────────────────┐
        │                                 │                                 │
 static / prerender               /_next/image                       dynamic dispatch
 (Rust, no V8)                    (Rust, no V8)                            │
 ssg, rsc,                        decode → resize → encode                 │
 _next/static (immutable),        + in-mem LRU + on-disk cache             │
 public                           (crates/nexide/src/image)                │
                                                                           ▼
                                                 ┌────────────────────────────────┐
                                                 │  EngineDispatcher              │
                                                 │  (crates/nexide/src/dispatch)  │
                                                 └────────────────┬───────────────┘
                                                                  │
                                                                  ▼
                                                 ┌────────────────────────────────┐
                                                 │  IsolatePool: hot V8 isolates  │
                                                 │  (crates/nexide/src/pool)      │
                                                 │  + per-isolate event pump      │
                                                 │  + RecyclePolicy (heap/req)    │
                                                 └────────────────┬───────────────┘
                                                                  │
                                                                  ▼
                                                 ┌────────────────────────────────┐
                                                 │  V8 isolate (raw rusty_v8)     │
                                                 │  + Node compat polyfills (JS)  │
                                                 │  + Rust ops bridge (op_*)      │
                                                 │  (crates/nexide/src/engine,    │
                                                 │   crates/nexide/runtime)       │
                                                 └────────────────────────────────┘
```

The runtime does **not** depend on `deno_core`, `deno_runtime`, `node-api`,
or `napi`. V8 is embedded directly via the [`v8`](https://crates.io/crates/v8)
crate (v147), which gives full control over isolate lifecycle, microtask
checkpoints, heap limits, and the op call ABI.

### Native `/_next/image` optimizer

Image optimization is the second hot-path that bypasses V8 entirely. Nexide
ships a Next.js-compatible `/_next/image` implementation in
`crates/nexide/src/image/` that mirrors Next 16.2.4 behaviour:

- **Validation** — same `images:` config from `next.config.mjs`
  (`deviceSizes`, `imageSizes`, `formats`, `qualities`, `domains`,
  `remotePatterns`, `localPatterns`, `dangerouslyAllowSVG`,
  `contentDispositionType`, `minimumCacheTTL`).
- **Source resolution** — local files under `public/` and `.next/static/`
  via a canonicalising `FsResolver`, plus remote fetch with allow-list
  matching (`picomatch`-style globs).
- **Pipeline** — decode (PNG/JPEG/WebP/GIF/AVIF/SVG), resize (Lanczos3),
  encode WebP/AVIF/JPEG/PNG with quality-aware quantisation
  (`imagequant` for PNG palette, `webp` for WebP, `ravif` for AVIF).
- **Caching** — bounded in-memory LRU (256 entries / 64 MB, zero-copy
  `bytes::Bytes`) in front of an SHA-256-keyed on-disk cache that respects
  the upstream `Cache-Control` and `minimumCacheTTL`. Repeat requests are
  answered with `x-nextjs-cache: HIT` without ever resolving the source.
- **`/_next/static` immutable** — Next.js's hashed asset folder is served
  with `Cache-Control: public, max-age=31536000, immutable` so browsers and
  CDNs can cache forever.

The benchmark above (`_next/image` row) measures exactly this path against
Next.js's own JS-based optimizer running in Node 22 / Deno 2.

## Repository layout

```
nexide/
├── crates/
│   ├── nexide/              ← the runtime crate
│   │   ├── src/
│   │   │   ├── server/      ← Axum HTTP shield, static asset routes,
│   │   │   │                  prerender hot path, accept loop
│   │   │   ├── engine/      ← V8 engine, CJS resolver, isolate trait
│   │   │   ├── ops/         ← Rust op_* implementations exposed to JS
│   │   │   │                  (fs, net, dns, tls, http_client, process,
│   │   │   │                   zlib, log, request/response, queue, …)
│   │   │   ├── pool/        ← isolate pool, worker, event pump strategies,
│   │   │   │                  recycle policies, mem sampler
│   │   │   ├── dispatch/    ← EngineDispatcher trait + errors
│   │   │   ├── cli.rs       ← `nexide start|bench|inspect` subcommands
│   │   │   └── lib.rs       ← public crate surface
│   │   ├── runtime/
│   │   │   └── polyfills/   ← JS shipped into every isolate
│   │   │       ├── node/    ← node:* modules (path, fs, http, crypto, …)
│   │   │       ├── async_local_storage.js
│   │   │       ├── cjs_loader.js
│   │   │       ├── http_bridge.js
│   │   │       ├── nexide_bridge.js
│   │   │       └── web_apis.js
│   │   └── tests/           ← integration tests (266 total)
│   ├── nexide-bench/        ← bench harness: local + docker-suite
│   └── nexide-e2e/          ← end-to-end tests against real Next.js
├── e2e/
│   ├── next-fixture/        ← Next.js 16 reference app used in tests + bench
│   └── prisma-sqlite/       ← Prisma library engine (N-API) + SQLite fixture
├── docs/                    ← design docs and historic task breakdowns
├── DESCRIPTION.md           ← original feasibility study
├── README.md                ← this file
├── LICENSE-MIT              ← dual licensed
├── LICENSE-APACHE           ← dual licensed
├── SECURITY.md              ← responsible disclosure policy
├── CONTRIBUTING.md          ← how to contribute
└── CODE_OF_CONDUCT.md       ← contributor covenant
```

## Node.js compatibility

Nexide ships its own implementations of the Node.js surface that Next.js
actually uses. Both `require('node:foo')` and `require('foo')` resolve to the
same instance.

| Module               | Status      | Notes                                              |
|----------------------|-------------|----------------------------------------------------|
| `path`               | full        | POSIX + Win32, picked from `process.platform`      |
| `path/posix` / `path/win32` | full | always-platform variants (Node parity)             |
| `url`                | full        | `URL`, `URLSearchParams`, legacy parse/format      |
| `querystring`        | full        | parse / stringify / escape / unescape              |
| `punycode`           | full        | RFC 3492 (vendored upstream `punycode.js` v2.1.0)  |
| `util`               | pragmatic   | `format`, `inspect`, `promisify`, `callbackify`    |
| `util/types`         | full        | re-export of `util.types`                          |
| `assert` / `assert/strict` | full  | strict-equality semantics by default               |
| `os`                 | full        | live + injectable backend (`OsInfoSource`)         |
| `events`             | full        | `EventEmitter` + `once` / `on` static helpers      |
| `buffer` / `Buffer`  | full        | UTF-8 / base64 / hex / latin1 / ascii / ucs2       |
| `stream`             | core        | `Readable` / `Writable` / `Duplex` / `Transform`   |
| `stream/web`         | full        | re-export of WHATWG Streams from `globalThis`      |
| `stream/promises`    | full        | promise-returning `pipeline` / `finished`          |
| `stream/consumers`   | full        | `buffer` / `text` / `json` / `arrayBuffer` / `blob`|
| `string_decoder`     | full        | UTF-8 / UTF-16LE / Latin-1 multi-chunk safe        |
| `fs` + `fs/promises` | sandboxed   | path sandbox; only configured roots are admitted   |
| `zlib`               | full        | gzip, deflate, brotli (sync + async wrappers)      |
| `crypto`             | core        | sha1/256/512, md5, HMAC, AES-256-GCM, randomUUID   |
| `http` / `https`     | server-side | enough for Next.js standalone server entrypoint    |
| `http2`              | stub        | loads + constants; `createServer`/`connect` throw  |
| `net` / `tls`        | client-side | enough for outbound `fetch` and DB drivers         |
| `dns` / `dns/promises` | full      | uses Tokio's resolver via Rust ops                 |
| `diagnostics_channel`| full        | `Channel` + `TracingChannel` (undici, OTel, APMs)  |
| `readline` / `readline/promises` | functional | line buffering over any Readable; no TTY UI |
| `child_process`      | core        | `spawn` / `exec` with stdio piping                 |
| `worker_threads`     | not supported | throws on `new Worker(...)`                      |
| `vm`                 | full        | real `v8::Context` per sandbox; not a security boundary |
| `async_hooks`        | ALS only    | `AsyncLocalStorage` works; full hooks do not       |
| `perf_hooks`         | core        | monotonic clock, basic marks                       |
| `timers` / `timers/promises` | full | backed by Tokio                                    |
| `inspector` / `tty` / `v8` / `module` / `constants` | core | enough surface for transitive deps |

| Global               | Status      | Notes                                              |
|----------------------|-------------|----------------------------------------------------|
| `process.env`        | whitelisted | `NEXT_*`, `NODE_*`, `NEXT_PUBLIC_*` + a few extras |
| `process.cwd/exit/hrtime/nextTick` | full | `exit` is recorded in `OpState`, never aborts host |
| `process.platform/.arch` | full    | Node-compatible strings                            |
| `Buffer`             | full        | global, identical to `require('buffer').Buffer`    |
| `setTimeout` / `setInterval` / `setImmediate` | full | timers backed by Tokio                |
| `queueMicrotask`     | full        | V8 native                                          |
| `fetch`, `Headers`, `Request`, `Response` | full | WHATWG Fetch                              |
| `ReadableStream`, `WritableStream`, `TransformStream` | full | WHATWG Streams                  |
| `TextEncoder` / `TextDecoder`, `URL`, `URLSearchParams` | full | WHATWG                          |
| `crypto.subtle` / `crypto.getRandomValues` / `crypto.randomUUID` | full | WebCrypto subset            |

## Known limitations

Nexide is a V8-only Next.js runtime; some Node platform surfaces are
intentionally absent or partial. The full list — N-API/native addons,
`http2`, worker threads, inspector, ESM at runtime, source maps,
corporate proxies, log rotation, etc. — lives in
[`docs/known-limitations.md`](docs/known-limitations.md).

If you hit something that isn't on that list, open an issue with the
exact `MODULE_NOT_FOUND` / runtime error and a minimal repro.

## Docker

Pre-built multi-arch images (`linux/amd64`, `linux/arm64`) are published to
the GitHub Container Registry on every tagged release:

```bash
docker pull ghcr.io/sonquer/nexide:latest
# or pin a version:
docker pull ghcr.io/sonquer/nexide:0.1.0
```

The image is Alpine-based (`alpine:3.20` runtime) with `gcompat` providing
the glibc shim needed by V8. The binary listens on `0.0.0.0:3000` by default
and serves the Next.js standalone bundle mounted at `/app`.

```bash
# build your Next.js app first (produces .next/standalone)
pnpm --dir e2e/next-fixture install
pnpm --dir e2e/next-fixture build

# run nexide against it
docker run --rm -p 3000:3000 \
    -v "$(pwd)/e2e/next-fixture:/app:ro" \
    ghcr.io/sonquer/nexide:latest
```

To build the image locally:

```bash
docker build -t nexide:dev .
docker run --rm -p 3000:3000 -v "$(pwd)/e2e/next-fixture:/app:ro" nexide:dev
```

## Migrating from Node.js or Deno

Nexide uses Next.js's standard `output: 'standalone'` build artefact, so the
build pipeline does not change. Only the runtime stage of your Dockerfile
needs swapping. Make sure your `next.config.js` has:

```js
module.exports = {
  output: 'standalone',
};
```

### From Node.js

**Before** (typical Node.js Dockerfile):

```dockerfile
FROM node:22-alpine AS builder
WORKDIR /app
COPY package.json pnpm-lock.yaml ./
RUN corepack enable && pnpm install --frozen-lockfile
COPY . .
RUN pnpm build

FROM node:22-alpine AS runtime
WORKDIR /app
COPY --from=builder /app/.next/standalone ./
COPY --from=builder /app/.next/static ./.next/static
COPY --from=builder /app/public ./public
EXPOSE 3000
CMD ["node", "server.js"]
```

**After** (Nexide):

```dockerfile
FROM node:22-alpine AS builder
WORKDIR /app
COPY package.json pnpm-lock.yaml ./
RUN corepack enable && pnpm install --frozen-lockfile
COPY . .
RUN pnpm build

FROM ghcr.io/sonquer/nexide:latest AS runtime
WORKDIR /app
COPY --from=builder --chown=nexide:nexide /app/.next/standalone ./
COPY --from=builder --chown=nexide:nexide /app/.next/static ./.next/static
COPY --from=builder --chown=nexide:nexide /app/public ./public
EXPOSE 3000
# ENTRYPOINT ["/usr/local/bin/nexide"] and CMD ["start", "/app"] are
# inherited from the base image. Override CMD if you need to customise
# bind address or port (although usually setting HOSTNAME/PORT env vars
# is cleaner):
# CMD ["start", "/app", "--hostname", "0.0.0.0", "--port", "8080"]
```

What changed:

- Runtime image: `node:22-alpine` → `ghcr.io/sonquer/nexide:latest`.
- `CMD ["node", "server.js"]` is **replaced** by the inherited Nexide
  entrypoint `nexide start /app`. There is no `node` process - Nexide
  loads the standalone bundle directly into V8.
- `--chown=nexide:nexide` because the Nexide image runs as the non-root
  `nexide` user (uid 10001).

### From Deno

**Before** (typical Deno Dockerfile):

```dockerfile
FROM denoland/deno:2.0.0 AS builder
WORKDIR /app
COPY . .
RUN deno task build

FROM denoland/deno:2.0.0 AS runtime
WORKDIR /app
COPY --from=builder /app/.next/standalone ./
COPY --from=builder /app/.next/static ./.next/static
COPY --from=builder /app/public ./public
EXPOSE 3000
CMD ["deno", "run", "-A", "--unstable-node-modules-dir", "server.js"]
```

**After** (Nexide):

```dockerfile
FROM denoland/deno:2.0.0 AS builder
WORKDIR /app
COPY . .
RUN deno task build

FROM ghcr.io/sonquer/nexide:latest AS runtime
WORKDIR /app
COPY --from=builder --chown=nexide:nexide /app/.next/standalone ./
COPY --from=builder --chown=nexide:nexide /app/.next/static ./.next/static
COPY --from=builder --chown=nexide:nexide /app/public ./public
EXPOSE 3000
# ENTRYPOINT ["/usr/local/bin/nexide"] and CMD ["start", "/app"] are
# inherited from the base image.
```

What changed:

- Runtime image: `denoland/deno:2.0.0` → `ghcr.io/sonquer/nexide:latest`.
- `CMD ["deno", "run", "-A", "--unstable-node-modules-dir", "server.js"]`
  is **replaced** by the inherited Nexide entrypoint `nexide start /app`.
  No `-A` permission flags, no `--unstable-*` flags, no `node_modules`
  resolver toggles - Nexide loads the Next.js standalone bundle directly
  into V8 via its native HTTP server.
- `--chown=nexide:nexide` because Nexide runs as the non-root `nexide`
  user (uid 10001).

Nexide does not care which runtime produced the bundle, only that the
resulting `.next/standalone` directory is laid out correctly.

### Pinning to a specific Nexide version

Production images should pin a full version, not `latest`:

```dockerfile
FROM ghcr.io/sonquer/nexide:0.1.0 AS runtime
```

Nexide is pre-1.0 and the runtime ABI is unstable. A point release can
change which Node.js compatibility surface ships, which heap defaults are
applied, and which environment variables are read. Pin, then upgrade
deliberately.

### Environment variables

| Variable        | Default          | Effect                                                 |
|-----------------|------------------|--------------------------------------------------------|
| `HOSTNAME`      | `127.0.0.1` (`0.0.0.0` in the published image) | Bind address for `nexide start`. Override to `0.0.0.0` when running in a container. |
| `PORT`          | `3000`           | TCP port for `nexide start`.                           |
| `RUST_LOG`      | `info`           | Standard `tracing-subscriber` filter syntax.           |
| `NODE_ENV`      | inherited        | Forwarded into `process.env`; Next.js reads it.        |
| `NEXT_*`        | inherited        | Whitelisted into `process.env` for Next.js.            |
| `NEXT_PUBLIC_*` | inherited        | Whitelisted for client-side env injection.             |

The same flags can be passed on the CLI: `nexide start /app --hostname 0.0.0.0 --port 8080`.

## Building and testing

```bash
# full check, lint, test
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets

# release build
cargo build --release

# subcommands
./target/release/nexide start <next-app-dir>
./target/release/nexide bench <next-app-dir>
./target/release/nexide inspect <next-app-dir>
```

The workspace is configured with strict lints: `dead_code`, `missing_docs`,
`unused_imports`, `unused_variables`, `unused_mut`, `unreachable_code`,
`unreachable_pub` and `unsafe_op_in_unsafe_fn` are all `deny`. The bar for
landing a PR is "the workspace builds with zero warnings".

## Releasing

Releases are cut by pushing a semver tag to `main`. Everything else is
automated by `.github/workflows/release.yml`.

1. Make sure `main` is green (CI passing) and the working tree is clean.
2. Bump the workspace version in [`Cargo.toml`](./Cargo.toml):

   ```toml
   [workspace.package]
   version = "0.1.0"
   ```

3. Run `cargo build --release` once locally to refresh `Cargo.lock`, then
   commit:

   ```bash
   git add Cargo.toml Cargo.lock
   git commit -m "release: 0.1.0"
   git push origin main
   ```

4. Tag and push the tag:

   ```bash
   git tag -a v0.1.0 -m "v0.1.0"
   git push origin v0.1.0
   ```

5. The `Release` workflow builds a multi-arch Docker image and pushes it to
   `ghcr.io/sonquer/nexide` with the following tags:

   - `0.1.0` (full version)
   - `0.1` (major.minor, only for stable tags)
   - `0` (major, only for stable tags)
   - `latest` (only when the tag does **not** contain `-`, i.e. not a prerelease)

   It then creates a GitHub Release with auto-generated notes from the
   commit history since the previous tag.

### Prereleases

For prereleases use a hyphen-suffixed tag, e.g. `v0.2.0-rc.1`. The workflow
detects this and:

- Skips the `latest` tag.
- Marks the GitHub Release as a prerelease.

### Yanking a release

Container images on GHCR can be deleted via the package settings page or the
GitHub CLI:

```bash
gh api -X DELETE /user/packages/container/nexide/versions/<version-id>
```

GitHub Releases can be deleted from the Releases UI. Prefer publishing a
patch release over yanking unless the artefact is actively harmful.

## Roadmap

Short term, in rough priority order:

1. Tune the isolate pump strategy on small containers (1-2 vCPU) where
   coalesced wakeups currently lose to serial dispatch on tail latency.
2. Reduce baseline memory: each warm isolate carries ~50 MB of V8 state;
   investigate startup snapshots once they pay off in a Hot Isolate model.
3. Tighten the `node:http` outbound surface so `keep-alive` agents and
   streamed bodies match Node behaviour byte-for-byte.
4. Stabilise the embed API (`BootContext`, `FsHandle`, dispatcher trait).

Explicit non-goals:

- Replacing Node.js for general workloads. Use Node, Deno or Bun.
- A plugin / native addon ABI. There is no `node-api`, no NAPI, no `.node`
  modules. If a Next.js dependency needs one, it has to be ported or replaced.
- Supporting old Next.js versions. Tracking current stable is enough work.

## Contributing

See [CONTRIBUTING.md](./CONTRIBUTING.md). TL;DR: open an issue first if the
change is non-trivial, run `cargo fmt && cargo clippy && cargo test` before
pushing, follow the existing module conventions (no inline comments inside
function bodies, doc comments on every public item).

By participating you agree to abide by [CODE_OF_CONDUCT.md](./CODE_OF_CONDUCT.md).

## Security

Security issues should **not** be filed as public GitHub issues. See
[SECURITY.md](./SECURITY.md) for the responsible disclosure process.

## License

Dual licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](./LICENSE-APACHE) or
  <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](./LICENSE-MIT) or
  <http://opensource.org/licenses/MIT>)

at your option.

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.

## Acknowledgements

Nexide stands on the shoulders of [V8](https://v8.dev),
[`rusty_v8`](https://github.com/denoland/rusty_v8) (the same V8 binding the
Deno project maintains), [Tokio](https://tokio.rs),
[Axum](https://github.com/tokio-rs/axum), [Hyper](https://hyper.rs), and the
WHATWG / Node.js spec bodies whose work made any of this tractable.

## Star History

<a href="https://www.star-history.com/?repos=sonquer%2Fnexide&type=date&legend=top-left">
 <picture>
   <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/chart?repos=sonquer/nexide&type=date&theme=dark&legend=top-left" />
   <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/chart?repos=sonquer/nexide&type=date&legend=top-left" />
   <img alt="Star History Chart" src="https://api.star-history.com/chart?repos=sonquer/nexide&type=date&legend=top-left" />
 </picture>
</a>

## Contributors Hall of Fame

<a href="https://github.com/sonquer/nexide/graphs/contributors">
  <img src="https://contrib.rocks/image?repo=sonquer/nexide" alt="Nexide contributors" />
</a>