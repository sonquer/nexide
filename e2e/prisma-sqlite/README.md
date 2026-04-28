# Nexide × Prisma + SQLite (E2E fixture)

A minimal Next.js 16 app that exercises Prisma's **library engine** (the
N-API native addon) against SQLite through `Nexide`.

What it covers:

- `@prisma/client` library engine loads via Nexide's N-API subset
- `prisma migrate deploy` + seed populates `prisma/dev.db`
- SSR page (`/`) reads users + posts via Prisma
- API route (`/api/users`) returns JSON from Prisma
- `/api/ping` for readiness probing

## Build

```bash
pnpm install        # or npm/yarn
pnpm build          # generates client, migrates, seeds, then next build
```

That produces:

- `.next/standalone/server.js` — entrypoint Nexide will execute
- `.next/standalone/prisma/dev.db` — seeded SQLite DB
- `.next/standalone/node_modules/.prisma/client/*` — generated client + engine

## Run with Nexide

From the workspace root:

```bash
cargo build --release
NEXIDE_BIND=127.0.0.1:3000 ./target/release/nexide \
  e2e/prisma-sqlite/.next/standalone/server.js
```

Then:

```bash
curl -s http://127.0.0.1:3000/api/users | jq
```

You should see two seeded users with their post counts.

## Run as a Rust e2e test

The `nexide-e2e` crate wires this up as an `#[ignore]`-gated test:

```bash
# 1. build the fixture (only needed once unless schema/seed change)
( cd e2e/prisma-sqlite && pnpm install && pnpm build )

# 2. release-build nexide
cargo build --release

# 3. run the e2e
cargo test -p nexide-e2e --release prisma_sqlite -- --ignored --nocapture
```
