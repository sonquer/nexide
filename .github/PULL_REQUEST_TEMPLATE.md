<!--
Thanks for the PR. Please fill in the sections below. Sections that do not
apply can be deleted, but do not delete the headings you do use.
-->

## What

A one or two line summary of what this PR changes.

## Why

The motivation. Link the issue this resolves: `Closes #123`.

## How

The shape of the change. Mention any non-obvious decisions, trade-offs, or
alternatives considered.

## Testing

How did you validate this change? List unit tests, integration tests,
benchmark numbers, or manual reproduction steps.

```text
cargo test --all-targets
cargo clippy --all-targets --all-features -- -D warnings
```

## Checklist

- [ ] `cargo fmt --all` is clean.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` is clean.
- [ ] `cargo test --all-targets` passes.
- [ ] New public items have doc comments.
- [ ] No inline comments inside function bodies (doc comments only).
- [ ] No `#[allow(dead_code)]` added.
- [ ] If this changes user-visible behaviour, the README or docs are updated.
