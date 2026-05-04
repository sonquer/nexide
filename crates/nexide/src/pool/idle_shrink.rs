//! Process-wide registry of idle-time RAM shrinkers.
//!
//! Long-lived caches (prerender, static RAM, image hot cache) register
//! a callback here at construction time. When the pump's idle path
//! fires, it walks the registry and asks each cache to evict its
//! contents. Subsequent requests refill the cache lazily from disk —
//! we trade a few extra `fs::read` calls for `O(100 MiB)` of RSS shed
//! during quiet periods.
//!
//! Each callback is a `Box<dyn Fn() + Send + Sync>` capturing an
//! `Arc<Cache>`; the Arc keeps the cache alive for the lifetime of
//! the process, so we never observe a stale closure.

use std::sync::OnceLock;
use std::time::Instant;

use parking_lot::Mutex;

type Shrinker = Box<dyn Fn() + Send + Sync + 'static>;

struct Registry {
    shrinkers: Vec<Shrinker>,
    last_run: Option<Instant>,
}

static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();

fn registry() -> &'static Mutex<Registry> {
    REGISTRY.get_or_init(|| {
        Mutex::new(Registry {
            shrinkers: Vec::new(),
            last_run: None,
        })
    })
}

/// Registers `shrink` to be invoked whenever the pump enters its idle
/// shrink path. Safe to call from any thread.
pub fn register<F: Fn() + Send + Sync + 'static>(shrink: F) {
    registry().lock().shrinkers.push(Box::new(shrink));
}

/// Invokes every registered shrinker. Returns the number invoked.
pub fn shrink_all() -> usize {
    let mut guard = registry().lock();
    guard.last_run = Some(Instant::now());
    for f in &guard.shrinkers {
        (f)();
    }
    guard.shrinkers.len()
}

/// Telemetry hook (Query) — last time `shrink_all` was invoked.
#[cfg(test)]
pub fn last_run() -> Option<Instant> {
    registry().lock().last_run
}

/// Test helper — clears the registry between unit tests so cases stay
/// isolated. Not exposed in release builds.
#[cfg(test)]
pub fn reset_for_tests() {
    let mut g = registry().lock();
    g.shrinkers.clear();
    g.last_run = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn shrink_all_invokes_every_registered_callback() {
        reset_for_tests();
        let counter = Arc::new(AtomicUsize::new(0));
        let c1 = Arc::clone(&counter);
        register(move || {
            c1.fetch_add(1, Ordering::SeqCst);
        });
        let c2 = Arc::clone(&counter);
        register(move || {
            c2.fetch_add(10, Ordering::SeqCst);
        });
        assert_eq!(shrink_all(), 2);
        assert_eq!(counter.load(Ordering::SeqCst), 11);
        assert!(last_run().is_some());
        reset_for_tests();
    }
}
