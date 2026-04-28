# Nexide × Next.js (E2E fixture)

Reference Next.js 16 app used by `crates/nexide-e2e` and `crates/nexide-bench`.
Not a "demo" — it's the regression matrix that gates releases.

## What it covers

| Route | Rendering mode | What it exercises |
|---|---|---|
| `/` | SSG (prerendered) | Rust static hot path, client-side Counter component |
| `/about` | SSG | Prerender cache with `x-nextjs-cache: HIT` |
| `/posts`, `/posts/[slug]` | ISR (revalidate 60s) | Incremental static regeneration |
| `/users`, `/users/[id]` | SSG + `generateStaticParams` | Static params with `dynamicParams=false` |
| `/api/ping` | Route handler (GET) | V8 isolate dispatch, JSON response |
| `/api/time` | Route handler (force-dynamic) | Dynamic V8 execution on every request |
| `/api/echo` | Route handler (POST) | Request body parsing through the Rust bridge |
| `/api/headers` | Route handler (GET) | Header introspection |
| `/forms` | Client component + POST | Full client-to-V8 round-trip |
| `/image-bench` | `next/image` | Native Rust image optimizer |

## Running with Nexide

```bash
# build the Next.js standalone bundle
pnpm install && pnpm build

# start with Nexide (from repo root)
cargo run --release -- start e2e/next-fixture --port 3000
```

## Running with Node.js (for comparison)

```bash
node e2e/next-fixture/.next/standalone/server.js
```

## Running the Rust e2e tests

From the workspace root:

```bash
cargo build --release
cargo test -p nexide-e2e --release -- --ignored --test-threads=1
```
