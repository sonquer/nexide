//! Process-level resident memory sampler.
//!
//! Provides a small abstraction (Dependency Inversion) so the
//! recycler can ask "how big is this process right now?" without
//! pulling a platform-specific dependency into every caller. The
//! production wiring uses [`ProcessSampler::live`]; tests use
//! [`MockSampler`].
//!
//! Note on semantics: RSS is always *process-wide* on Linux/macOS.
//! When several workers share one process the value cannot be
//! attributed per worker — the recycle policy interpreting it
//! treats the threshold as a process-wide budget shared across
//! workers (each worker checks the same value and any of them may
//! trigger the recycle). This is good enough for the headline
//! "kill the process before it OOMs the container" use case;
//! multi-process deployments achieve true per-worker RSS
//! attribution naturally because each process has exactly one
//! worker.

use std::sync::Mutex;

/// Single point-in-time memory observation expressed in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemorySample {
    /// Resident set size in bytes.
    pub rss_bytes: u64,
}

/// Read-only contract for sampling process memory.
///
/// Implementations are pure Queries (Command/Query Separation) —
/// they observe state but never mutate it — and must be safe to
/// call from any thread because the recycler invokes them from
/// the pool's coordination task.
pub trait MemorySampler: Send + Sync + 'static {
    /// Returns the current memory snapshot, or `None` when the
    /// platform cannot supply one (e.g. macOS dev build with the
    /// Linux-only `/proc` reader). `None` makes the calling
    /// recycle policy a no-op rather than a hard error.
    fn sample(&self) -> Option<MemorySample>;
}

/// In-memory sampler used by tests.
///
/// Pre-loaded with a fixed series of values; each call to
/// [`MemorySampler::sample`] consumes one entry. After the script
/// is exhausted further calls return the last value (so a test
/// asserting "fires after N samples" can keep polling without
/// surprising `None`s).
pub struct MockSampler {
    series: Mutex<Vec<u64>>,
    last: Mutex<Option<u64>>,
}

impl MockSampler {
    /// Builds a sampler whose subsequent calls return the bytes in
    /// `series` in order. An empty `series` yields `None` for every
    /// call.
    #[must_use]
    pub fn new(series: Vec<u64>) -> Self {
        Self {
            series: Mutex::new(series.into_iter().rev().collect()),
            last: Mutex::new(None),
        }
    }
}

impl MemorySampler for MockSampler {
    fn sample(&self) -> Option<MemorySample> {
        let mut series = self.series.lock().expect("MockSampler poisoned");
        let mut last = self.last.lock().expect("MockSampler poisoned");
        if let Some(next) = series.pop() {
            *last = Some(next);
            Some(MemorySample { rss_bytes: next })
        } else {
            last.map(|rss_bytes| MemorySample { rss_bytes })
        }
    }
}

/// Production sampler reading the host's process accounting.
///
/// On Linux the constructor returns a sampler that parses
/// `/proc/self/status::VmRSS`. On other platforms (macOS dev
/// builds, Windows) the sampler always returns `None` so the
/// recycle policy disables itself silently — there is no portable
/// libc shortcut and shelling out to `ps` would add latency to a
/// hot-path call.
pub struct ProcessSampler;

impl ProcessSampler {
    /// Returns the canonical live sampler boxed as
    /// `Arc<dyn MemorySampler>`.
    #[must_use]
    pub fn live() -> std::sync::Arc<dyn MemorySampler> {
        std::sync::Arc::new(Self)
    }
}

#[cfg(target_os = "linux")]
impl MemorySampler for ProcessSampler {
    fn sample(&self) -> Option<MemorySample> {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("VmRSS:") {
                let kib = rest.split_whitespace().next()?.parse::<u64>().ok()?;
                return Some(MemorySample {
                    rss_bytes: kib.saturating_mul(1024),
                });
            }
        }
        None
    }
}

#[cfg(not(target_os = "linux"))]
impl MemorySampler for ProcessSampler {
    fn sample(&self) -> Option<MemorySample> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_sampler_returns_each_value_in_order() {
        let sampler = MockSampler::new(vec![100, 200, 300]);
        assert_eq!(sampler.sample().map(|s| s.rss_bytes), Some(100));
        assert_eq!(sampler.sample().map(|s| s.rss_bytes), Some(200));
        assert_eq!(sampler.sample().map(|s| s.rss_bytes), Some(300));
    }

    #[test]
    fn mock_sampler_repeats_last_after_exhaustion() {
        let sampler = MockSampler::new(vec![42]);
        assert_eq!(sampler.sample().map(|s| s.rss_bytes), Some(42));
        assert_eq!(sampler.sample().map(|s| s.rss_bytes), Some(42));
        assert_eq!(sampler.sample().map(|s| s.rss_bytes), Some(42));
    }

    #[test]
    fn mock_sampler_returns_none_for_empty_series() {
        let sampler = MockSampler::new(Vec::new());
        assert!(sampler.sample().is_none());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn live_sampler_returns_some_on_linux() {
        let sampler = ProcessSampler;
        let sample = sampler
            .sample()
            .expect("/proc available on linux test runner");
        assert!(sample.rss_bytes > 0, "test process must have non-zero RSS");
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn live_sampler_returns_none_off_linux() {
        let sampler = ProcessSampler;
        assert!(sampler.sample().is_none());
    }
}
