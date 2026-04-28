//! DIP boundary between the runtime and the JavaScript engine.
//!
//! The trait is intentionally narrow - boot a worker, advance its event
//! loop one step, query heap diagnostics - so that production code and
//! tests can swap the heavy `V8Engine` for an in-memory mock without
//! pulling V8 into the test build.

use std::path::Path;

use async_trait::async_trait;

use super::EngineError;

/// Snapshot of V8 heap diagnostics taken at a single point in time.
///
/// All values are bytes. The struct is `Copy` so callers can freely
/// pass it across thread boundaries (e.g. into a metrics pipeline).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeapStats {
    /// Bytes allocated by V8 to back live JavaScript objects.
    pub used_heap_size: usize,

    /// Total bytes reserved by V8 (committed memory, including slack).
    pub total_heap_size: usize,

    /// Hard ceiling configured for this isolate.
    pub heap_size_limit: usize,
}

impl HeapStats {
    /// Returns the fraction of the configured heap limit currently in
    /// use, clamped to `0.0..=1.0`.
    ///
    /// Returns `0.0` when the heap limit is zero - this only happens
    /// for synthetic stats produced by tests; real V8 isolates always
    /// report a positive limit.
    #[must_use]
    pub fn utilization(&self) -> f64 {
        if self.heap_size_limit == 0 {
            return 0.0;
        }
        let raw = used_over_limit(self.used_heap_size, self.heap_size_limit);
        raw.clamp(0.0, 1.0)
    }
}

#[allow(clippy::cast_precision_loss)]
fn used_over_limit(used: usize, limit: usize) -> f64 {
    used as f64 / limit as f64
}

/// Behavioural contract every JavaScript engine must satisfy.
///
/// The trait is `async` and object-safe via [`async_trait`] so that
/// downstream layers can hold `Box<dyn IsolateHandle>` across the
/// worker pool. The trait is
/// `?Send` because V8 isolates are pinned to a single OS thread.
#[async_trait(?Send)]
pub trait IsolateHandle {
    /// Boots the engine with the given entrypoint script.
    ///
    /// # Errors
    ///
    /// * [`EngineError::Bootstrap`] - V8 platform / runtime construction
    ///   failed.
    /// * [`EngineError::ModuleResolution`] - entrypoint path cannot be
    ///   converted to a module specifier or does not exist.
    /// * [`EngineError::JsRuntime`] - the entrypoint parsed but threw
    ///   during evaluation.
    async fn boot(entrypoint: &Path) -> Result<Self, EngineError>
    where
        Self: Sized;

    /// Drives the underlying event loop until it has no more work
    /// pending or it errors out.
    ///
    /// # Errors
    ///
    /// [`EngineError::JsRuntime`] when an unhandled exception or rejected
    /// promise propagates out of the event loop.
    async fn pump(&mut self) -> Result<(), EngineError>;

    /// Returns the most recently observed heap snapshot.
    ///
    /// Implementations are required to keep this cheap (no V8 lock
    /// acquisition) and side-effect free so that callers can poll it
    /// from observability paths.
    fn heap_stats(&self) -> HeapStats;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utilization_is_zero_for_zero_limit() {
        let stats = HeapStats {
            used_heap_size: 10,
            total_heap_size: 10,
            heap_size_limit: 0,
        };
        assert!((stats.utilization() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn utilization_is_clamped_to_unit_interval() {
        let stats = HeapStats {
            used_heap_size: 1_000,
            total_heap_size: 1_000,
            heap_size_limit: 100,
        };
        assert!((stats.utilization() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn utilization_is_monotonic_in_used_heap() {
        let limit = 1_024;
        let small = HeapStats {
            used_heap_size: 128,
            total_heap_size: 256,
            heap_size_limit: limit,
        };
        let large = HeapStats {
            used_heap_size: 512,
            total_heap_size: 768,
            heap_size_limit: limit,
        };
        assert!(small.utilization() < large.utilization());
        assert!(small.utilization() > 0.0);
        assert!(large.utilization() < 1.0);
    }
}
