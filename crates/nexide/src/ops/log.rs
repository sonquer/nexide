//! Worker identity threaded through the runtime for log de-duplication.
//!
//! `console.log` / `process.stdout.write` calls from every isolate
//! converge on a single tracing pipeline. To avoid `N×` duplicated
//! Next.js boot banners (one per worker), each isolate carries a
//! [`WorkerId`] in its [`crate::engine::v8_engine::BridgeState`] and
//! the print/log ops gate `info`/`debug`/`trace` output to the
//! primary worker. `warn` and `error` always pass through.

/// Identity of the isolate currently running JavaScript.
#[derive(Debug, Clone, Copy)]
pub struct WorkerId {
    /// Stable ordinal of the worker — `0` for the primary, monotonic
    /// across the pool.
    pub id: usize,
    /// `true` when this worker should emit `console.log/info/debug`
    /// output, `false` when those levels should be suppressed.
    pub is_primary: bool,
}

impl WorkerId {
    /// Constructs an identity. The convention used by the production
    /// runtime is `is_primary = (id == 0)` so only the first worker
    /// prints the Next.js boot banner.
    #[must_use]
    pub const fn new(id: usize, is_primary: bool) -> Self {
        Self { id, is_primary }
    }
}
