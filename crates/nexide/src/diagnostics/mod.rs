//! Lightweight diagnostics: contention counters + periodic logger.

pub(crate) mod contention;

use std::time::Duration;

const LOGGER_INTERVAL: Duration = Duration::from_secs(5);

/// Spawn a task that logs contention counter deltas every 5s.
/// The handle is intentionally not awaited: cancelling it stops
/// logging.
pub fn spawn_periodic_logger() -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut prev = contention::Snapshot::current();
        let mut ticker = tokio::time::interval(LOGGER_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        ticker.tick().await;
        loop {
            ticker.tick().await;
            let now = contention::Snapshot::current();
            let delta = now.delta(&prev);
            if delta.has_activity() {
                tracing::info!(
                    target: "nexide::contention",
                    prerender_read_fast = delta.prerender_read_fast,
                    prerender_read_contended = delta.prerender_read_contended,
                    prerender_write_fast = delta.prerender_write_fast,
                    prerender_write_contended = delta.prerender_write_contended,
                    ram_cache_fast = delta.ram_cache_fast,
                    ram_cache_contended = delta.ram_cache_contended,
                    mem_cache_fast = delta.mem_cache_fast,
                    mem_cache_contended = delta.mem_cache_contended,
                    inflight_acquire_fast = delta.inflight_acquire_fast,
                    inflight_acquire_contended = delta.inflight_acquire_contended,
                    "contention sample"
                );
            }
            prev = now;
        }
    })
}
