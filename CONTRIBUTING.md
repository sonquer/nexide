# Contributing to Nexide

Thank you for considering a contribution. This document describes the
mechanics; the philosophy is "small, well-tested, well-documented changes
that don't fight the existing architecture."

## Before you start

For anything beyond a typo fix or a one-line bug fix, **open an issue first**.
We would rather discuss the approach for ten minutes than ask you to throw
away a 500-line PR. State clearly:

- What problem you are solving.
- The smallest reasonable scope to solve it.
- The trade-off you are accepting.

## Development setup

Requirements:

- Rust 1.85 or newer (we use edition 2024).
- A C++ toolchain (V8 build prerequisites: `clang`, `python3`, `pkg-config`).
- `pnpm` or `npm`, only if you are touching `example/` or running E2E tests.

Clone, then:

```bash
cargo build
cargo test --all-targets
```

The first build is slow because the `v8` crate downloads or builds V8 itself.
Subsequent builds are incremental.

## Project conventions

These are non-negotiable; PRs that violate them will be sent back for changes.

### Rust

- **No dead code, period.** The workspace lints `dead_code = "deny"`. If a
  helper has no caller, delete it. Adding `#[allow(dead_code)]` is not an
  acceptable workaround.
- **Doc comments on every public item.** The workspace lints
  `missing_docs = "deny"`.
- **No inline comments inside function bodies.** Doc comments (`///`,
  `//!`, `/** */`) explain *what* and *why*. `// SAFETY:` blocks explaining
  invariants required by `unsafe` are the only allowed exception.
- **No commented-out code.** Use git history.
- **SOLID and CQS.** Prefer small focused types with one reason to change.
  Separate command methods (mutating, return `()` or `Result<(), _>`) from
  query methods (pure, return data).
- **Tests live next to the code** (`#[cfg(test)] mod tests` in the same
  file) for unit tests, or in `tests/` for integration tests. New behaviour
  needs a test; new bug fixes need a regression test.
- **Errors are typed.** Use `thiserror` for library error enums. Avoid
  `anyhow::Error` outside of the binary entrypoint and tests.
- **`unsafe` requires a `// SAFETY:` comment** explaining the invariants
  the caller must uphold and how this call site upholds them.

### JavaScript polyfills

- **No inline comments inside function bodies.** Use JSDoc on functions and
  module-level block comments at the top of the file.
- **No empty `try / catch`.** If you genuinely need to swallow an error,
  log it via the `_trace()` helper or document why silent is correct.
- **Prefer `catch { }` (no binding) over `catch (_) { }`** for intentionally
  swallowed errors.
- **Match Node.js semantics, not just shape.** A function that accepts the
  same arguments but throws a different error type will break Next.js in
  unexpected ways downstream.

## Workflow

1. Fork the repo, branch off `main`.
2. Make focused commits. We rebase before merge; messy histories will be
   squashed.
3. Run the full check before pushing:

   ```bash
   cargo fmt --all
   cargo clippy --all-targets --all-features -- -D warnings
   cargo test --all-targets
   ```

4. Open a PR. Fill in the template. Link the issue you are resolving.
5. A maintainer will review. Expect questions; the review is collaborative,
   not adversarial.

## Commit messages

We follow a relaxed Conventional Commits style:

```
<area>: <imperative summary>

Optional body explaining why, not what.
Wrap at ~72 columns.
```

Examples:

- `pool: drop coalesced pump strategy on 1-vCPU presets`
- `node/path: fix posix normalization when input ends in '/..'`
- `bench: emit RPS/CPU% column in docker-suite output`

## What is in scope

- Performance improvements with reproducible benchmarks.
- Node.js compatibility fixes that unblock real Next.js applications.
- New `node:*` module surface that Next.js or commonly-used middleware
  depends on.
- Test coverage for previously untested paths.
- Documentation, examples, error messages.

## What is out of scope

- General-purpose JavaScript runtime features that Next.js does not need.
- Plugin / native addon ABIs (no NAPI, no `.node` modules).
- Support for Next.js versions older than the current stable.
- Style-only changes (renaming things, reformatting) without a behavioural
  reason.

If you are unsure, ask in an issue first.
