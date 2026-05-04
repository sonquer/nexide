# Known limitations

Things that **do not** work in nexide today and will bite a Next.js
production deploy if you rely on them. Each item is a deliberate
trade-off (V8-only runtime, no full Node platform), not a missing-feature
backlog item.

## Native addons (`.node` files / N-API / node-gyp)

Nexide implements a substantial subset of the N-API ABI directly against
V8 — enough for many Next.js–adjacent addons to load and run. What
currently works:

- **Values & types**: `napi_get_*` / `napi_create_*` for primitives,
  strings (UTF-8), objects, arrays, dates, BigInt, externals.
- **Properties**: get/set named, define properties with attributes,
  prototype chain.
- **Functions & classes**: `napi_create_function`, `napi_define_class`
  (with property descriptors — methods, values, static), `napi_wrap` /
  `napi_unwrap`, `napi_new_instance`, `napi_call_function`,
  `napi_get_cb_info`.
- **Errors**: `napi_throw_*`, `napi_create_error`, type/range error
  variants, pending exception propagation, `napi_fatal_error` /
  `napi_fatal_exception`.
- **Buffers & typed arrays**: `napi_create_buffer*`,
  `napi_create_typedarray`, `napi_create_arraybuffer`, finalizers (with
  caveats — backing-store deleters fired off-thread by V8 currently
  pass `napi_env=NULL`).
- **References**: `napi_create_reference`, `_ref` / `_unref` / `_value` /
  `_delete`.
- **Promises & deferreds**: `napi_create_promise`,
  `napi_resolve_deferred`, `napi_reject_deferred` — single-shot
  resolvers driven from JS or N-API thread callbacks.
- **BigInt**: `napi_create_bigint_int64` / `_uint64` / `_words` and the
  matching `napi_get_value_bigint_*` getters.
- **Async work**: `napi_create_async_work` / `_queue` / `_cancel` /
  `_delete` — execute runs on tokio's blocking pool, complete is
  trampolined back to the V8 thread (the engine pump is woken so
  callbacks fire even on otherwise-idle isolates).
- **Threadsafe-functions**: `napi_create_threadsafe_function`,
  `napi_call_threadsafe_function`, `_acquire` / `_release` /
  `_get_context` / `_ref` / `_unref` — calls from any thread are
  funnelled through the engine pump and dispatched under a real V8
  scope. Cross-thread wake-up is wired so the pump comes out of idle as
  soon as a worker thread pushes a callback.

Confirmed working end-to-end (see `e2e/prisma-sqlite/`):

- **`@prisma/client` library engine** (`libquery_engine-*.dylib.node`)
  against SQLite — full N-API path: tsfn-driven async query pipeline,
  promise-based connect / query, BigInt cursors, `process.dlopen`,
  Node-style subpath imports (`#main-entry-point`).

What still doesn't work:

- **`sharp`** — `next/image` doesn't need it (nexide's native
  `/_next/image` optimizer covers the built-in route). Custom loaders
  that call `require('sharp')` directly still fail.
- **`canvas`** (node-canvas) and other addons that link non-trivial
  third-party C++ libraries (Cairo, Pango, …) — the surface they
  consume is much wider than what's implemented above.
- **Anything using Node's `uv_*` / libuv API directly** (some database
  drivers, FFI bridges) — N-API lives entirely above libuv, so addons
  that bypass it are out of scope.

For pure-JS swap-ins still recommended where they exist:

- **`bcrypt`** native → `bcryptjs`.
- **`better-sqlite3`** / `sqlite3` → use `@prisma/client` (works,
  see above), an HTTP-fronted SQLite (Turso, D1), or a pure-JS driver.

## HTTP/2 server

`require('node:http2')` exposes a working **client** subset:
`http2.connect(authority)` opens a real h2 session via Rust ops, and
`session.request(headers)` returns a `Http2Stream` you can write/read
like a Duplex (enough for gRPC clients that go through `@grpc/grpc-js`'s
HTTP/2 channel, REST clients that opportunistically upgrade, etc.).

The **server** side (`http2.createServer`, `createSecureServer`) is not
implemented — nexide's shield terminates HTTP/1.1 + h2 itself and hands
requests to JS as a Node-shaped `IncomingMessage`/`ServerResponse`
regardless of the wire protocol, so user code does not need a separate
h2 server. Most deps probe via `try { require('http2') } catch {}` and
fall back, so this is usually transparent.

## Inspector / debugger protocol

`require('node:inspector')` ships a small APM-probe shim: a
`Session` whose `post(method, params, cb)` answers a curated set of
calls that Datadog / Sentry / Elastic agents issue at startup —
`Runtime.evaluate` (via `vm.runInThisContext`), `Runtime.getHeapUsage`,
`Runtime.getHeapStatistics`, `HeapProfiler.collectGarbage`, plus
acknowledged-but-no-op `Profiler.enable`/`disable`. Every other method
rejects with the standard `-32601` "Method not found" error, mirroring
the Inspector wire format.

The full DevTools protocol (live debugger, sampling CPU profiler,
incremental heap snapshots) is **not** implemented. Capture profiles
and heap snapshots externally — e.g. via `kill -USR1` on stock Node
during local repro, or via the host process / sidecar in production.

## Worker threads

`require('node:worker_threads')` resolves but `new Worker(...)` throws.
Next.js uses workers for ISR background revalidation; in nexide that
path is serialised onto the request loop. For most workloads this is
invisible; if you have heavy CPU-bound revalidation, scale horizontally
instead.

## `cluster`, `dgram`, `repl`, `domain`, `wasi`, `trace_events`

Not shipped. These are server-side primitives nexide replaces with
native equivalents (multi-process scaling via `SO_REUSEPORT` instead of
`cluster`, OpenTelemetry instead of `trace_events`, etc.) or that are
simply not used by the Next.js server runtime.

## ESM at runtime

Next.js standalone bundles every dependency as CommonJS via webpack, so
`import` statements that survive bundling are extremely rare. Pure-ESM
packages that ship `.mjs` and rely on dynamic `import()` at runtime are
not currently routed through nexide's resolver. Workaround: pin the
package to a version that still provides a CJS build, or pre-bundle it
into your app code via webpack's `transpilePackages`.

## Source maps in stack traces

Stack frames currently show positions inside the bundled chunk
(`/.next/server/chunks/3660.js:2:78064`) rather than original sources.
Wire your error reporter (Sentry, Datadog) with the build's `.map` files
the same way you would for stock Node.

## Custom CA / corporate proxies

`NODE_EXTRA_CA_CERTS` is honoured by the outbound `https` polyfill but
proxy-related env vars (`HTTP_PROXY`, `HTTPS_PROXY`, `NO_PROXY`) are not
threaded through `fetch` / `https.request` yet. Set them on a sidecar
proxy if you need outbound proxy support.

## Crash-consistent log rotation

`SIGUSR2` is not handled (`pino`/`winston` rotation hooks are silently
no-ops). Rotate via the orchestrator (Kubernetes log driver, journald)
instead of inside the app.

---

If you hit a limitation that isn't on this list — open an issue with the
exact `MODULE_NOT_FOUND` / runtime error and a minimal repro. Everything
above started life as a production bug report.
