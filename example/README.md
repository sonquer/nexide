# nexide example app

A Next.js 16 application used as the reference target for the nexide runtime.

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

## Running with nexide

```bash
# build the Next.js standalone bundle
pnpm install && pnpm build

# start with nexide (from repo root)
cargo run --release -- start example --port 3000
```

## Running with Node.js (for comparison)

```bash
node example/.next/standalone/server.js
```
