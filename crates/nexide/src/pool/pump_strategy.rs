//! Pluggable per-isolate request pump strategy.
//!
//! Two implementations ship today:
//!
//! * [`Serial`] - the original behaviour. The JS pump awaits one
//!   id at a time via `op_nexide_pop_request`. Cheapest under low
//!   concurrency; one op crossing per request.
//! * [`Coalesced`] - pump-coalescing optimisation. The JS pump awaits a slice
//!   via `op_nexide_pop_request_batch(max)` and dispatches every id
//!   in the slice within the same microtask cycle. Amortises the
//!   per-request op crossing under sustained load.
//!
//! Selection is configured by [`pump_strategy_from_env`] reading
//! `NEXIDE_PUMP_BATCH`:
//!
//! | env value           | strategy                         |
//! |---------------------|----------------------------------|
//! | unset / `""` / `"0"`| [`Serial`] (default)           |
//! | positive integer    | [`Coalesced`] with that batch cap  |
//!
//! The trait + factory are deliberately small: the strategy only
//! reports its **name** (for tracing) and its **JS pump source**
//! (the snippet installed at boot inside `nexide_bridge.js`).
//! Behaviour lives in JavaScript because the V8 microtask queue
//! is the actual scheduler - Rust just decides which pump variant
//! to install.

use std::fmt;

/// Default and maximum batch caps for the [`Coalesced`] strategy.
///
/// The default keeps the cap small enough that batches stay within
/// a single microtask budget; the ceiling matches the safety bound
/// on the op side ([`crate::ops::extension::op_nexide_pop_request_batch`]).
pub const DEFAULT_BATCH: u32 = 32;
/// Hard upper bound on the configurable batch cap. Mirrors the
/// op-side ceiling.
pub const MAX_BATCH: u32 = 256;

/// SOLID seam between the runtime and the JS pump implementation.
///
/// Implementors are tiny value types - no state, no allocation.
/// The `'static` bound is intentional: strategies may be cloned
/// into tracing fields and JS source strings.
pub trait PumpStrategy: fmt::Debug + Send + Sync + 'static {
    /// Stable, lowercase tag for tracing / metrics.
    fn name(&self) -> &'static str;

    /// Hint of the in-flight cap the strategy expects to handle in
    /// a single microtask. `1` for serial pumps; the batch cap for
    /// batched pumps.
    fn max_inflight_per_tick(&self) -> u32;
}

/// Original serial pump (`await op_nexide_pop_request()` in a loop).
#[derive(Debug, Clone, Copy, Default)]
pub struct Serial;

impl PumpStrategy for Serial {
    fn name(&self) -> &'static str {
        "serial"
    }

    fn max_inflight_per_tick(&self) -> u32 {
        1
    }
}

/// Coalesced pump (`await op_nexide_pop_request_batch(cap)`).
#[derive(Debug, Clone, Copy)]
pub struct Coalesced {
    cap: u32,
}

impl Coalesced {
    /// Constructs a batched strategy with `cap` clamped to
    /// `1..=MAX_BATCH`.
    #[must_use]
    pub fn new(cap: u32) -> Self {
        Self {
            cap: cap.clamp(1, MAX_BATCH),
        }
    }

    /// Configured batch cap (post-clamp).
    #[must_use]
    pub const fn cap(&self) -> u32 {
        self.cap
    }
}

impl Default for Coalesced {
    fn default() -> Self {
        Self::new(DEFAULT_BATCH)
    }
}

impl PumpStrategy for Coalesced {
    fn name(&self) -> &'static str {
        "coalesced"
    }

    fn max_inflight_per_tick(&self) -> u32 {
        self.cap
    }
}

/// Type-erased boxed strategy used by the worker boot path.
pub type BoxedPumpStrategy = Box<dyn PumpStrategy>;

/// Selects a strategy from the `NEXIDE_PUMP_BATCH` value.
///
/// Returns [`Serial`] when the env var is unset, empty, `"0"` or
/// unparseable. Otherwise returns [`Coalesced`] with the parsed cap
/// clamped to `1..=MAX_BATCH`.
///
/// phase B note: the unset case is no longer the runtime
/// default for multi-thread mode - see [`default_pump_strategy_for`]
/// which decides the *implicit* default per worker mode. Operators
/// who export `NEXIDE_PUMP_BATCH=0` still get the legacy serial
/// pump on every worker.
#[must_use]
pub fn pump_strategy_from_env(value: Option<&str>) -> Option<BoxedPumpStrategy> {
    match value.map(str::trim) {
        None => None,
        Some("" | "0") => Some(Box::new(Serial)),
        Some(raw) => match raw.parse::<u32>() {
            Ok(0) | Err(_) => Some(Box::new(Serial)),
            Ok(cap) => Some(Box::new(Coalesced::new(cap))),
        },
    }
}

/// Implicit default pump strategy per worker mode (phase B).
///
/// * `single-thread` → [`Serial`]: a lone worker has no
///   concurrency to amortise, and waiting for a batch to fill in
///   `nexide_bridge.js` only adds latency to the first request of
///   each tick (`docker-suite` 1cpu/256 `api-time` p99 jumps from
///   65 ms to 87 ms when batched is forced on a single worker).
/// * `multi-thread` → [`Coalesced`] with [`DEFAULT_BATCH`]: sustained
///   load with ≥ 2 workers benefits from amortising the
///   per-request `op_nexide_pop_request` crossing.
///
/// Pure helper so the policy is unit-testable without booting V8.
#[must_use]
pub(super) fn default_pump_strategy_for(workers: usize) -> BoxedPumpStrategy {
    if workers <= 1 {
        Box::new(Serial)
    } else {
        let cap = (workers as u32).clamp(1, MAX_BATCH);
        Box::new(Coalesced::new(cap))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_by_one_advertises_single_inflight() {
        let s = Serial;
        assert_eq!(s.name(), "serial");
        assert_eq!(s.max_inflight_per_tick(), 1);
    }

    #[test]
    fn batched_clamps_zero_to_one() {
        let s = Coalesced::new(0);
        assert_eq!(s.cap(), 1);
    }

    #[test]
    fn batched_clamps_above_max() {
        let s = Coalesced::new(10_000);
        assert_eq!(s.cap(), MAX_BATCH);
    }

    #[test]
    fn batched_default_uses_default_constant() {
        let s = Coalesced::default();
        assert_eq!(s.cap(), DEFAULT_BATCH);
        assert_eq!(s.max_inflight_per_tick(), DEFAULT_BATCH);
        assert_eq!(s.name(), "coalesced");
    }

    #[test]
    fn env_unset_returns_none() {
        let s = pump_strategy_from_env(None);
        assert!(s.is_none());
    }

    #[test]
    fn env_empty_returns_serial() {
        let s = pump_strategy_from_env(Some("")).expect("strategy");
        assert_eq!(s.name(), "serial");
    }

    #[test]
    fn env_zero_returns_serial() {
        let s = pump_strategy_from_env(Some("0")).expect("strategy");
        assert_eq!(s.name(), "serial");
    }

    #[test]
    fn env_garbage_returns_serial() {
        let s = pump_strategy_from_env(Some("not-a-number")).expect("strategy");
        assert_eq!(s.name(), "serial");
    }

    #[test]
    fn env_positive_returns_batched_with_clamped_cap() {
        let s = pump_strategy_from_env(Some("16")).expect("strategy");
        assert_eq!(s.name(), "coalesced");
        assert_eq!(s.max_inflight_per_tick(), 16);
    }

    #[test]
    fn env_huge_value_clamps_to_max() {
        let s = pump_strategy_from_env(Some("999999")).expect("strategy");
        assert_eq!(s.name(), "coalesced");
        assert_eq!(s.max_inflight_per_tick(), MAX_BATCH);
    }

    #[test]
    fn default_for_single_worker_is_serial() {
        let s = default_pump_strategy_for(1);
        assert_eq!(s.name(), "serial");
    }

    #[test]
    fn default_for_zero_workers_is_serial() {
        let s = default_pump_strategy_for(0);
        assert_eq!(s.name(), "serial");
    }

    #[test]
    fn default_for_multi_worker_is_batched_with_cap_eq_workers() {
        for n in [2usize, 4, 8, 16, 64] {
            let s = default_pump_strategy_for(n);
            assert_eq!(s.name(), "coalesced", "workers={n}");
            assert_eq!(s.max_inflight_per_tick(), n as u32, "workers={n}");
        }
    }

    #[test]
    fn default_clamps_cap_to_max_batch() {
        let s = default_pump_strategy_for((MAX_BATCH as usize) * 4);
        assert_eq!(s.name(), "coalesced");
        assert_eq!(s.max_inflight_per_tick(), MAX_BATCH);
    }
}
