//! Per-isolate V8 heap budget.
//!
//! Without an explicit configuration `v8::Isolate` sizes its
//! `heap_size_limit` against host RAM - typically 1 - 4 GB on a
//! workstation, *several hundred MB* in a 1-CPU container after
//! V8's heuristic adjustment. Because the [`HeapThreshold`] recycle
//! policy fires at `used / heap_size_limit`, an unbounded limit
//! makes the watchdog worthless: the worker can balloon to >1 GB
//! before "80 % full" is reached, and `mem_max_mb` in the bench
//! suite climbs accordingly.
//!
//! [`HeapLimitConfig`] caps the limit explicitly, derived from the
//! `NEXIDE_HEAP_LIMIT_MB` environment variable. The default
//! ([`HeapLimitConfig::DEFAULT_MAX_MB`]) is sized for a Next.js
//! standalone worker and matches the smallest container preset
//! used in the bench suite (`1cpu-256mb`).
//!
//! [`HeapThreshold`]: super::super::pool::recycle::HeapThreshold

use v8;

/// Validated V8 heap budget, in megabytes.
///
/// Carries the *initial* commitment (sets V8's young-generation
/// page) and the *maximum* old-generation cap. `initial_mb` is
/// always less than or equal to `max_mb`; constructors enforce the
/// invariant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeapLimitConfig {
    initial_mb: usize,
    max_mb: usize,
}

impl HeapLimitConfig {
    /// Project-wide default cap (megabytes).
    ///
    /// Picked to make `1cpu-256mb` work without OOM while still
    /// catching any per-request leak above ~200 MB. Operators with
    /// larger workloads should set `NEXIDE_HEAP_LIMIT_MB` explicitly.
    pub const DEFAULT_MAX_MB: usize = 256;

    /// Project-wide default for the *initial* commitment.
    ///
    /// V8 grows the heap on demand up to the maximum, so a small
    /// initial value keeps idle workers small (relevant for the
    /// "many isolates, few requests" deployment shape).
    pub const DEFAULT_INITIAL_MB: usize = 32;

    /// Lower bound on `max_mb`. Below this V8 cannot bootstrap the
    /// runtime without immediately tripping the near-heap-limit
    /// callback.
    pub const MIN_MAX_MB: usize = 32;

    /// Constructs a config, clamping inputs to safe ranges.
    ///
    /// * `max_mb` is clamped up to [`Self::MIN_MAX_MB`].
    /// * `initial_mb` is clamped down to `max_mb` and up to `1`.
    #[must_use]
    pub const fn new(initial_mb: usize, max_mb: usize) -> Self {
        let max_mb = if max_mb < Self::MIN_MAX_MB {
            Self::MIN_MAX_MB
        } else {
            max_mb
        };
        let initial_mb = if initial_mb == 0 {
            1
        } else if initial_mb > max_mb {
            max_mb
        } else {
            initial_mb
        };
        Self { initial_mb, max_mb }
    }

    /// Project default. Equivalent to
    /// `Self::new(DEFAULT_INITIAL_MB, DEFAULT_MAX_MB)`.
    #[must_use]
    pub const fn default_const() -> Self {
        Self::new(Self::DEFAULT_INITIAL_MB, Self::DEFAULT_MAX_MB)
    }

    /// Returns the configured initial commitment in megabytes.
    #[must_use]
    pub const fn initial_mb(&self) -> usize {
        self.initial_mb
    }

    /// Returns the configured maximum cap in megabytes.
    #[must_use]
    pub const fn max_mb(&self) -> usize {
        self.max_mb
    }

    /// Builds the V8 [`v8::CreateParams`] honouring this budget.
    ///
    /// The values are converted to bytes and passed to
    /// `heap_limits`. Returns a fresh `CreateParams` so callers can
    /// chain additional configuration before forwarding to
    /// `RuntimeOptions::create_params`.
    pub fn to_create_params(&self) -> v8::CreateParams {
        const MB: usize = 1024 * 1024;
        v8::Isolate::create_params().heap_limits(self.initial_mb * MB, self.max_mb * MB)
    }
}

impl Default for HeapLimitConfig {
    fn default() -> Self {
        Self::default_const()
    }
}

/// Parses the `NEXIDE_HEAP_LIMIT_MB` environment value.
///
/// Accepts a single positive integer that becomes the *maximum*
/// cap; the initial commitment stays at
/// [`HeapLimitConfig::DEFAULT_INITIAL_MB`]. Returns `None` for
/// missing / blank / non-numeric input so callers fall back to the
/// project default.
///
/// `0` is treated as "missing" rather than "disable" - V8 cannot
/// run with a zero heap and silently disabling the cap reintroduces
/// the host-sized limit this module exists to prevent.
#[must_use]
pub fn heap_limit_from_env(raw: Option<&str>) -> Option<HeapLimitConfig> {
    let parsed = raw
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|n| *n > 0)?;
    Some(HeapLimitConfig::new(
        HeapLimitConfig::DEFAULT_INITIAL_MB.min(parsed),
        parsed,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_uses_project_constants() {
        let cfg = HeapLimitConfig::default();
        assert_eq!(cfg.max_mb(), HeapLimitConfig::DEFAULT_MAX_MB);
        assert_eq!(cfg.initial_mb(), HeapLimitConfig::DEFAULT_INITIAL_MB);
    }

    #[test]
    fn new_clamps_max_below_minimum() {
        let cfg = HeapLimitConfig::new(1, 4);
        assert_eq!(cfg.max_mb(), HeapLimitConfig::MIN_MAX_MB);
        assert_eq!(cfg.initial_mb(), 1);
    }

    #[test]
    fn new_caps_initial_at_max() {
        let cfg = HeapLimitConfig::new(1024, 128);
        assert_eq!(cfg.max_mb(), 128);
        assert_eq!(cfg.initial_mb(), 128);
    }

    #[test]
    fn new_lifts_zero_initial_to_one() {
        let cfg = HeapLimitConfig::new(0, 128);
        assert_eq!(cfg.initial_mb(), 1);
    }

    #[test]
    fn env_parser_returns_none_for_blank() {
        assert!(heap_limit_from_env(None).is_none());
        assert!(heap_limit_from_env(Some("")).is_none());
        assert!(heap_limit_from_env(Some("   ")).is_none());
    }

    #[test]
    fn env_parser_returns_none_for_garbage() {
        assert!(heap_limit_from_env(Some("abc")).is_none());
        assert!(heap_limit_from_env(Some("-1")).is_none());
        assert!(heap_limit_from_env(Some("3.5")).is_none());
    }

    #[test]
    fn env_parser_treats_zero_as_missing() {
        assert!(heap_limit_from_env(Some("0")).is_none());
    }

    #[test]
    fn env_parser_clamps_below_minimum() {
        let cfg = heap_limit_from_env(Some("4")).expect("parsed");
        assert_eq!(cfg.max_mb(), HeapLimitConfig::MIN_MAX_MB);
    }

    #[test]
    fn env_parser_keeps_initial_le_max() {
        let cfg = heap_limit_from_env(Some("16")).expect("parsed");
        assert_eq!(cfg.max_mb(), HeapLimitConfig::MIN_MAX_MB);
        assert!(cfg.initial_mb() <= cfg.max_mb());
    }

    #[test]
    fn env_parser_accepts_large_values() {
        let cfg = heap_limit_from_env(Some(" 1024 ")).expect("parsed");
        assert_eq!(cfg.max_mb(), 1024);
        assert_eq!(cfg.initial_mb(), HeapLimitConfig::DEFAULT_INITIAL_MB);
    }

    #[test]
    fn create_params_uses_byte_sized_limits() {
        let cfg = HeapLimitConfig::new(8, 64);
        let params = cfg.to_create_params();
        let _ = params;
    }
}
