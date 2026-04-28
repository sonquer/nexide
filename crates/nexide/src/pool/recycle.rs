//! Worker recycling policies.
//!
//! Each policy maps a [`WorkerHealth`] snapshot to a boolean
//! "recycle now?" decision. Composition is open for extension via
//! [`Composite`] (Open/Closed Principle) — new criteria can be added
//! without touching existing implementations.

use std::sync::Arc;

use super::mem_sampler::MemorySampler;
use super::worker::WorkerHealth;

/// Decision contract used by [`super::IsolatePool`] after every
/// dispatch.
///
/// Implementations must be cheap (called on the hot path) and pure
/// (Query — same input must yield the same output).
pub trait RecyclePolicy: Send + Sync + 'static {
    /// Returns `true` when the pool should retire the worker that
    /// produced `snapshot`.
    fn should_recycle(&self, snapshot: &WorkerHealth) -> bool;
}

/// Recycle when the V8 heap utilisation strictly exceeds the
/// configured ratio.
///
/// `ratio` is clamped to `[0.0, 1.0]` at construction so the policy
/// cannot be configured into pathological always-on / never-on states.
#[derive(Debug, Clone, Copy)]
pub struct HeapThreshold {
    ratio: f64,
}

impl HeapThreshold {
    /// Constructs a [`HeapThreshold`] that fires when used heap /
    /// heap limit exceeds `ratio`.
    #[must_use]
    pub const fn new(ratio: f64) -> Self {
        Self {
            ratio: ratio.clamp(0.0, 1.0),
        }
    }

    /// Returns the configured ratio.
    #[must_use]
    pub const fn ratio(&self) -> f64 {
        self.ratio
    }
}

impl RecyclePolicy for HeapThreshold {
    fn should_recycle(&self, snapshot: &WorkerHealth) -> bool {
        let limit = snapshot.heap.heap_size_limit;
        if limit == 0 {
            return false;
        }
        #[allow(clippy::cast_precision_loss)]
        let used = snapshot.heap.used_heap_size as f64;
        #[allow(clippy::cast_precision_loss)]
        let limit_f = limit as f64;
        used / limit_f > self.ratio
    }
}

/// Recycle once the worker has handled `max` requests.
#[derive(Debug, Clone, Copy)]
pub struct RequestCount {
    max: u64,
}

impl RequestCount {
    /// Constructs a [`RequestCount`] policy with the given limit.
    #[must_use]
    pub const fn new(max: u64) -> Self {
        Self { max }
    }

    /// Returns the configured limit.
    #[must_use]
    pub const fn max(&self) -> u64 {
        self.max
    }
}

impl RecyclePolicy for RequestCount {
    fn should_recycle(&self, snapshot: &WorkerHealth) -> bool {
        snapshot.requests_handled >= self.max
    }
}

/// Recycle when the V8 *used heap* in bytes exceeds the configured cap.
///
/// Complements [`HeapThreshold`] with an absolute (rather than
/// relative-to-`heap_size_limit`) trigger so deployments that
/// shrink the heap budget via `NEXIDE_HEAP_LIMIT_MB` keep a tight
/// cap regardless of the dynamic limit V8 reports.
#[derive(Debug, Clone, Copy)]
pub struct HeapBytes {
    max_bytes: usize,
}

impl HeapBytes {
    /// Constructs a [`HeapBytes`] policy that fires when used heap
    /// exceeds `max_bytes`.
    #[must_use]
    pub const fn new(max_bytes: usize) -> Self {
        Self { max_bytes }
    }

    /// Returns the configured byte cap.
    #[must_use]
    pub const fn max_bytes(&self) -> usize {
        self.max_bytes
    }
}

impl RecyclePolicy for HeapBytes {
    fn should_recycle(&self, snapshot: &WorkerHealth) -> bool {
        snapshot.heap.used_heap_size > self.max_bytes
    }
}

/// Recycle when the *process-wide* RSS reported by a
/// [`MemorySampler`] exceeds `max_bytes`.
///
/// Useful as a last-resort guard against off-heap leaks (typed
/// arrays, buffers held by Rust ops) that the V8 heap sensor
/// cannot see. Because RSS is process-scoped, all workers in a
/// shared-process deployment will read the same value and either
/// of them may trigger the recycle — that is intentional: the
/// goal is to keep the *container* under its memory limit, and any
/// worker getting recycled releases V8 allocations associated with
/// its isolate.
pub struct ProcessRss {
    max_bytes: u64,
    sampler: Arc<dyn MemorySampler>,
}

impl ProcessRss {
    /// Constructs a [`ProcessRss`] policy with the provided sampler.
    #[must_use]
    pub fn new(max_bytes: u64, sampler: Arc<dyn MemorySampler>) -> Self {
        Self { max_bytes, sampler }
    }

    /// Returns the configured cap in bytes.
    #[must_use]
    pub const fn max_bytes(&self) -> u64 {
        self.max_bytes
    }
}

impl RecyclePolicy for ProcessRss {
    fn should_recycle(&self, _snapshot: &WorkerHealth) -> bool {
        self.sampler
            .sample()
            .is_some_and(|s| s.rss_bytes > self.max_bytes)
    }
}

/// Composite "any-of" policy: triggers when any of its inner policies
/// votes for recycling.
pub struct Composite {
    policies: Vec<Arc<dyn RecyclePolicy>>,
}

impl Composite {
    /// Builds a composite from owned policy handles.
    #[must_use]
    pub const fn new(policies: Vec<Arc<dyn RecyclePolicy>>) -> Self {
        Self { policies }
    }
}

impl RecyclePolicy for Composite {
    fn should_recycle(&self, snapshot: &WorkerHealth) -> bool {
        self.policies.iter().any(|p| p.should_recycle(snapshot))
    }
}

/// Parses `NEXIDE_REAP_HEAP_RATIO` content.
///
/// * `None` — variable missing, blank, non-numeric, or negative.
///   Caller falls back to a built-in default (or "disabled" semantics
///   if no default is configured).
/// * `Some(0.0)` — operator explicitly disables the heap-watchdog
///   policy (no [`HeapThreshold`] is added to the [`Composite`]).
/// * `Some(r)` for `0.0 < r ≤ 1.0` — fire when used heap exceeds `r`.
/// * Values strictly greater than `1.0` are clamped by [`HeapThreshold`]
///   itself to `1.0`, never panicking.
#[must_use]
pub fn reap_heap_ratio_from_env(raw: Option<&str>) -> Option<f64> {
    raw.map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|r| r.is_finite() && *r >= 0.0)
}

/// Parses `NEXIDE_REAP_AFTER_REQUESTS` content.
///
/// * `None` — variable missing, blank, or non-numeric.
/// * `Some(0)` — operator explicitly disables the request-count policy.
/// * `Some(n)` for `n ≥ 1` — fire after `n` handled requests.
#[must_use]
pub fn reap_request_count_from_env(raw: Option<&str>) -> Option<u64> {
    raw.map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse::<u64>().ok())
}

/// Parses `NEXIDE_REAP_HEAP_MB` content (megabytes).
///
/// Returns `None` for missing / blank / non-numeric input. `Some(0)`
/// is preserved verbatim so the caller can interpret it as
/// "explicitly disabled".
#[must_use]
pub fn reap_heap_bytes_from_env(raw: Option<&str>) -> Option<usize> {
    raw.map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse::<usize>().ok())
        .map(|mb| mb.saturating_mul(1024 * 1024))
}

/// Parses `NEXIDE_REAP_RSS_MB` content (megabytes).
///
/// Same semantics as [`reap_heap_bytes_from_env`]. The result is
/// interpreted by [`ProcessRss::new`] which expects bytes.
#[must_use]
pub fn reap_rss_bytes_from_env(raw: Option<&str>) -> Option<u64> {
    raw.map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse::<u64>().ok())
        .map(|mb| mb.saturating_mul(1024 * 1024))
}

/// Builds the production [`Composite`] policy from the parsed env
/// configuration, applying the project-wide defaults when an env var
/// is absent.
///
/// Defaults: `heap_ratio = 0.8`, `request_count = 50_000`. Either
/// component is **omitted** (not constructed with a degenerate `0`)
/// when the corresponding env value is exactly `0`, because policies
/// such as [`RequestCount::new(0)`] would fire on every dispatch and
/// turn the recycler into a busy loop.
///
/// Returning a zero-policy [`Composite`] (both disabled) is well
/// defined — [`Composite::should_recycle`] returns `false` for an
/// empty policy list, so the recycler simply never fires.
#[must_use]
pub fn build_default_recycle_policy(
    heap_ratio: Option<f64>,
    request_count: Option<u64>,
) -> Arc<dyn RecyclePolicy> {
    build_default_recycle_policy_with(heap_ratio, request_count, None, None, None)
}

/// Extended builder accepting the additional byte-cap policies.
///
/// `heap_bytes` (env `NEXIDE_REAP_HEAP_MB`) and `rss_bytes` (env
/// `NEXIDE_REAP_RSS_MB`) are independently optional; passing
/// `Some(0)` disables that component just like the existing
/// ratio/count toggles. `sampler` is required when `rss_bytes` is
/// supplied — when omitted the RSS policy is silently skipped so
/// callers don't have to special-case the macOS dev path where
/// [`ProcessSampler`](super::mem_sampler::ProcessSampler) returns
/// no data anyway.
#[must_use]
pub fn build_default_recycle_policy_with(
    heap_ratio: Option<f64>,
    request_count: Option<u64>,
    heap_bytes: Option<usize>,
    rss_bytes: Option<u64>,
    sampler: Option<Arc<dyn MemorySampler>>,
) -> Arc<dyn RecyclePolicy> {
    const DEFAULT_HEAP_RATIO: f64 = 0.8;
    const DEFAULT_REQUEST_COUNT: u64 = 50_000;
    let heap = heap_ratio.unwrap_or(DEFAULT_HEAP_RATIO);
    let requests = request_count.unwrap_or(DEFAULT_REQUEST_COUNT);
    let mut policies: Vec<Arc<dyn RecyclePolicy>> = Vec::new();
    if heap > 0.0 {
        policies.push(Arc::new(HeapThreshold::new(heap)));
    }
    if requests > 0 {
        policies.push(Arc::new(RequestCount::new(requests)));
    }
    if let Some(cap) = heap_bytes.filter(|n| *n > 0) {
        policies.push(Arc::new(HeapBytes::new(cap)));
    }
    if let (Some(cap), Some(s)) = (rss_bytes.filter(|n| *n > 0), sampler) {
        policies.push(Arc::new(ProcessRss::new(cap, s)));
    }
    Arc::new(Composite::new(policies))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::HeapStats;

    fn health(used: usize, limit: usize, handled: u64) -> WorkerHealth {
        WorkerHealth {
            heap: HeapStats {
                used_heap_size: used,
                total_heap_size: used,
                heap_size_limit: limit,
            },
            requests_handled: handled,
        }
    }

    #[test]
    fn heap_threshold_does_not_fire_at_or_below_threshold() {
        let policy = HeapThreshold::new(0.8);
        assert!(!policy.should_recycle(&health(0, 100, 0)));
        assert!(!policy.should_recycle(&health(80, 100, 0)));
    }

    #[test]
    fn heap_threshold_fires_strictly_above_threshold() {
        let policy = HeapThreshold::new(0.8);
        assert!(policy.should_recycle(&health(81, 100, 0)));
        assert!(policy.should_recycle(&health(99, 100, 0)));
    }

    #[test]
    fn heap_threshold_treats_zero_limit_as_no_information() {
        let policy = HeapThreshold::new(0.5);
        assert!(!policy.should_recycle(&health(1024, 0, 0)));
    }

    #[test]
    fn heap_threshold_clamps_ratio_to_unit_range() {
        assert!((HeapThreshold::new(2.0).ratio() - 1.0).abs() < f64::EPSILON);
        assert!((HeapThreshold::new(-0.5).ratio() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn request_count_fires_at_and_above_threshold() {
        let policy = RequestCount::new(50);
        assert!(!policy.should_recycle(&health(0, 0, 49)));
        assert!(policy.should_recycle(&health(0, 0, 50)));
        assert!(policy.should_recycle(&health(0, 0, 100)));
    }

    #[test]
    fn composite_fires_when_any_inner_policy_votes() {
        let policy = Composite::new(vec![
            Arc::new(HeapThreshold::new(0.99)),
            Arc::new(RequestCount::new(10)),
        ]);
        assert!(!policy.should_recycle(&health(50, 100, 5)));
        assert!(policy.should_recycle(&health(50, 100, 11)));
        assert!(policy.should_recycle(&health(100, 100, 0)));
    }

    #[test]
    fn composite_with_no_policies_never_recycles() {
        let policy = Composite::new(Vec::new());
        assert!(!policy.should_recycle(&health(99, 100, 999_999)));
    }

    #[test]
    fn reap_heap_ratio_from_env_accepts_well_formed_values() {
        assert_eq!(reap_heap_ratio_from_env(Some("0")), Some(0.0));
        assert_eq!(reap_heap_ratio_from_env(Some("0.5")), Some(0.5));
        assert_eq!(reap_heap_ratio_from_env(Some(" 0.95 ")), Some(0.95));
        assert_eq!(reap_heap_ratio_from_env(Some("1")), Some(1.0));
        assert_eq!(reap_heap_ratio_from_env(Some("1.5")), Some(1.5));
    }

    #[test]
    fn reap_heap_ratio_from_env_rejects_invalid_values() {
        assert_eq!(reap_heap_ratio_from_env(None), None);
        assert_eq!(reap_heap_ratio_from_env(Some("")), None);
        assert_eq!(reap_heap_ratio_from_env(Some("   ")), None);
        assert_eq!(reap_heap_ratio_from_env(Some("abc")), None);
        assert_eq!(reap_heap_ratio_from_env(Some("-0.5")), None);
        assert_eq!(reap_heap_ratio_from_env(Some("nan")), None);
        assert_eq!(reap_heap_ratio_from_env(Some("inf")), None);
    }

    #[test]
    fn reap_request_count_from_env_accepts_well_formed_values() {
        assert_eq!(reap_request_count_from_env(Some("0")), Some(0));
        assert_eq!(reap_request_count_from_env(Some("1")), Some(1));
        assert_eq!(reap_request_count_from_env(Some(" 50000 ")), Some(50_000));
    }

    #[test]
    fn reap_request_count_from_env_rejects_invalid_values() {
        assert_eq!(reap_request_count_from_env(None), None);
        assert_eq!(reap_request_count_from_env(Some("")), None);
        assert_eq!(reap_request_count_from_env(Some("-1")), None);
        assert_eq!(reap_request_count_from_env(Some("3.14")), None);
        assert_eq!(reap_request_count_from_env(Some("abc")), None);
    }

    #[test]
    fn build_default_recycle_policy_uses_project_defaults() {
        let policy = build_default_recycle_policy(None, None);
        assert!(!policy.should_recycle(&health(50, 100, 49_999)));
        assert!(policy.should_recycle(&health(50, 100, 50_000)));
        assert!(policy.should_recycle(&health(81, 100, 0)));
    }

    #[test]
    fn build_default_recycle_policy_omits_disabled_policies() {
        let only_heap = build_default_recycle_policy(Some(0.5), Some(0));
        assert!(!only_heap.should_recycle(&health(0, 0, 999_999)));
        assert!(only_heap.should_recycle(&health(60, 100, 0)));

        let only_requests = build_default_recycle_policy(Some(0.0), Some(10));
        assert!(!only_requests.should_recycle(&health(99, 100, 5)));
        assert!(only_requests.should_recycle(&health(99, 100, 10)));
    }

    #[test]
    fn build_default_recycle_policy_with_both_disabled_never_fires() {
        let policy = build_default_recycle_policy(Some(0.0), Some(0));
        assert!(!policy.should_recycle(&health(99, 100, u64::MAX)));
    }

    #[test]
    fn heap_bytes_fires_strictly_above_cap() {
        let policy = HeapBytes::new(1_000);
        assert!(!policy.should_recycle(&health(999, 0, 0)));
        assert!(!policy.should_recycle(&health(1_000, 0, 0)));
        assert!(policy.should_recycle(&health(1_001, 0, 0)));
    }

    #[test]
    fn process_rss_fires_when_sampler_exceeds_cap() {
        use crate::pool::mem_sampler::MockSampler;
        let sampler = Arc::new(MockSampler::new(vec![512, 2_048]));
        let policy = ProcessRss::new(1_024, sampler);
        assert!(!policy.should_recycle(&health(0, 0, 0)));
        assert!(policy.should_recycle(&health(0, 0, 0)));
    }

    #[test]
    fn process_rss_is_noop_when_sampler_returns_none() {
        use crate::pool::mem_sampler::MockSampler;
        let sampler = Arc::new(MockSampler::new(Vec::new()));
        let policy = ProcessRss::new(1, sampler);
        assert!(!policy.should_recycle(&health(0, 0, 0)));
    }

    #[test]
    fn reap_heap_bytes_from_env_parses_megabytes() {
        assert_eq!(reap_heap_bytes_from_env(None), None);
        assert_eq!(reap_heap_bytes_from_env(Some("")), None);
        assert_eq!(reap_heap_bytes_from_env(Some("abc")), None);
        assert_eq!(reap_heap_bytes_from_env(Some("0")), Some(0));
        assert_eq!(reap_heap_bytes_from_env(Some("1")), Some(1024 * 1024));
        assert_eq!(
            reap_heap_bytes_from_env(Some(" 256 ")),
            Some(256 * 1024 * 1024)
        );
    }

    #[test]
    fn reap_rss_bytes_from_env_parses_megabytes() {
        assert_eq!(
            reap_rss_bytes_from_env(Some("128")),
            Some(128 * 1024 * 1024)
        );
        assert_eq!(reap_rss_bytes_from_env(Some("0")), Some(0));
        assert_eq!(reap_rss_bytes_from_env(Some("-5")), None);
    }

    #[test]
    fn build_default_recycle_policy_with_includes_heap_bytes_when_configured() {
        let policy = build_default_recycle_policy_with(Some(0.0), Some(0), Some(1_000), None, None);
        assert!(!policy.should_recycle(&health(999, 0, 0)));
        assert!(policy.should_recycle(&health(2_000, 0, 0)));
    }

    #[test]
    fn build_default_recycle_policy_with_skips_rss_when_sampler_missing() {
        let policy = build_default_recycle_policy_with(Some(0.0), Some(0), None, Some(1), None);
        assert!(!policy.should_recycle(&health(0, 0, 0)));
    }
}
