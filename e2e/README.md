# E2E fixtures

Each subdirectory here is a real Next.js 16 application built with
`output: "standalone"` and exercised by the harness in
[`crates/nexide-e2e`](../crates/nexide-e2e). They are **not** demos —
they're the regression matrix that gates releases.

| Fixture | Purpose |
|---|---|
| [`next-fixture/`](./next-fixture) | Reference app: SSG, ISR, route handlers, client components, `next/image`, dynamic params |
| [`prisma-sqlite/`](./prisma-sqlite) | Prisma library engine (N-API) + SQLite + seed; validates that real native addons load and run |

## Running everything

From the workspace root:

```bash
# 1. build all fixtures
( cd e2e/next-fixture  && pnpm install && pnpm build )
( cd e2e/prisma-sqlite && pnpm install && pnpm build )

# 2. release-build nexide
cargo build --release

# 3. run e2e
cargo test -p nexide-e2e --release -- --ignored --test-threads=1
```

Each fixture's `README.md` documents how to boot it standalone for
manual inspection.

## Adding a new fixture

1. Create `e2e/<name>/` with a normal Next.js project + `output: "standalone"`.
2. Make sure `pnpm build` produces `.next/standalone/server.js`.
3. Add a path helper in [`crates/nexide-e2e/src/lib.rs`](../crates/nexide-e2e/src/lib.rs).
4. Add an `#[ignore]`-gated `#[tokio::test]` in `crates/nexide-e2e/tests/<name>.rs`
   that calls `NexideProcess::spawn_at(...)`.
5. Update this index.
