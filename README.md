# nexide

<p align="center">
  <img src="./img/nexide.png" alt="nexide logo" width="240" />
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

`nexide` embeds a pool of raw V8 isolates behind an Axum/Tower HTTP server, and
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
optimised for the Next.js critical path. `nexide` asks a narrower question:

> If we own the entire stack (HTTP, isolate lifecycle, Node API surface),
> how fast can a Next.js standalone bundle actually serve?

## Benchmarks

All numbers below come from `nexide-bench docker-suite`: 4 routes × 3 runtimes
× 4 container presets × 30 s window @ 64 connections, on identical Docker
images of the same Next.js 16 application.

Runtimes: `nexide` (this repo), `node` (Node 22 LTS standalone server),
`deno` (Deno 2.x with `--unstable-node-modules-dir`).

### 1 vCPU, 512 MB RAM (typical small container)

| Route        | Runtime | RPS    | p50    | p95    | p99    | Mem avg |
|--------------|---------|-------:|-------:|-------:|-------:|--------:|
| `api/ping`   | nexide  |  4 790 |  9.3ms | 43.0ms | 55.7ms |  196 MB |
| `api/ping`   | node    |  2 667 | 18.0ms | 45.3ms | 57.2ms |   57 MB |
| `api/ping`   | deno    |  3 094 | 13.7ms | 53.4ms | 64.8ms |   84 MB |
| `api/time`   | nexide  |  4 672 |  9.5ms | 43.2ms | 58.0ms |  158 MB |
| `api/time`   | node    |  2 604 | 18.2ms | 46.6ms | 68.7ms |   55 MB |
| `ssg/about`  | nexide  | 14 474 |  4.1ms |  6.2ms |  7.7ms |  218 MB |
| `ssg/about`  | node    |  3 511 | 14.6ms | 39.3ms | 49.7ms |   54 MB |
| `rsc/about`  | nexide  | 20 829 |  2.6ms |  5.0ms |  7.8ms |  218 MB |
| `rsc/about`  | node    |  3 548 | 14.1ms | 39.5ms | 50.4ms |   56 MB |

### Δ vs Node.js across all four container presets

| Preset           | api-ping RPS | api-time RPS | ssg RPS  | rsc RPS  |
|------------------|-------------:|-------------:|---------:|---------:|
| 1 vCPU /  256 MB |       +66.3% |       +58.1% |  +302.5% |  +472.9% |
| 1 vCPU /  512 MB |       +79.6% |       +79.4% |  +312.2% |  +487.0% |
| 1 vCPU / 1024 MB |       +57.9% |       +59.0% |  +289.6% |  +447.3% |
| 2 vCPU / 1024 MB |      +137.7% |      +144.8% |  +240.0% |  +381.9% |

### What this means honestly

- **Static / RSC routes are 3-5x faster.** This is where the static prerender
  hot path bypasses the V8 pool entirely.
- **Route handlers are ~60-80% faster on a single core, ~140% faster on two.**
  V8 dispatch overhead is the dominant cost; Rust does not magically make your
  JS faster, it makes the path *to* your JS shorter.
- **Memory cost is real.** `nexide` uses 2-4x the memory of Node.js because it
  pre-warms a pool of V8 isolates. This is the central trade-off.
- **CPU utilisation is broadly comparable.** No magic; the wins come from
  parallelism in the pool and a leaner request path, not from running V8 less.

Reproduce locally:

```bash
cargo run --release -p nexide-bench -- docker-suite \
    --routes api-ping,api-time,ssg-about,rsc-about \
    --runtimes nexide,node,deno \
    --presets 1cpu-512mb,2cpu-1024mb \
    --duration 30s --connections 64
```

## Quick start

Requirements: Rust 1.85+ (for edition 2024), a built Next.js standalone bundle.

```bash
# build the runtime
cargo build --release

# build the bundled example app
pnpm --dir example install
pnpm --dir example build

# serve it
./target/release/nexide start example --port 3000

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
                ┌─────────────────────────┴─────────────────────────┐
                │                                                   │
       static / prerender                                    dynamic dispatch
       (Rust, no V8)                                                │
       ssg, rsc, _next/static, public                               │
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
├── example/                 ← Next.js 16 reference app used in tests + bench
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

`nexide` ships its own implementations of the Node.js surface that Next.js
actually uses. Both `require('node:foo')` and `require('foo')` resolve to the
same instance.

| Module               | Status      | Notes                                              |
|----------------------|-------------|----------------------------------------------------|
| `path`               | full        | POSIX + Win32, picked from `process.platform`      |
| `url`                | full        | `URL`, `URLSearchParams`, legacy parse/format      |
| `querystring`        | full        | parse / stringify / escape / unescape              |
| `util`               | pragmatic   | `format`, `inspect`, `promisify`, `callbackify`    |
| `os`                 | full        | live + injectable backend (`OsInfoSource`)         |
| `events`             | full        | `EventEmitter` + `once` / `on` static helpers      |
| `buffer` / `Buffer`  | full        | UTF-8 / base64 / hex / latin1 / ascii / ucs2       |
| `stream`             | core        | `Readable` / `Writable` / `Duplex` / `Transform`   |
| `fs` + `fs/promises` | sandboxed   | path sandbox; only configured roots are admitted   |
| `zlib`               | full        | gzip, deflate, brotli (sync + async wrappers)      |
| `crypto`             | core        | sha1/256/512, md5, HMAC, AES-256-GCM, randomUUID   |
| `http` / `https`     | server-side | enough for Next.js standalone server entrypoint    |
| `net` / `tls`        | client-side | enough for outbound `fetch` and DB drivers         |
| `dns` / `dns/promises` | full      | uses Tokio's resolver via Rust ops                 |
| `child_process`      | core        | `spawn` / `exec` with stdio piping                 |
| `worker_threads`     | not supported | throws on `new Worker(...)`                      |
| `vm`                 | core        | `runInNewContext`, `compileFunction`               |
| `async_hooks`        | ALS only    | `AsyncLocalStorage` works; full hooks do not       |
| `perf_hooks`         | core        | monotonic clock, basic marks                       |

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
pnpm --dir example install
pnpm --dir example build

# run nexide against it
docker run --rm -p 3000:3000 \
    -v "$(pwd)/example:/app:ro" \
    ghcr.io/sonquer/nexide:latest
```

To build the image locally:

```bash
docker build -t nexide:dev .
docker run --rm -p 3000:3000 -v "$(pwd)/example:/app:ro" nexide:dev
```

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

`nexide` stands on the shoulders of [V8](https://v8.dev),
[`rusty_v8`](https://github.com/denoland/rusty_v8) (the same V8 binding the
Deno project maintains), [Tokio](https://tokio.rs),
[Axum](https://github.com/tokio-rs/axum), [Hyper](https://hyper.rs), and the
WHATWG / Node.js spec bodies whose work made any of this tractable.
