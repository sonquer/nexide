//! Shared building blocks for the two-task V8 pump pattern used by
//! both [`super::IsolateWorker`] (multi-thread mode) and
//! [`super::LocalIsolateWorker`] (single-thread mode).
//!
//! ## Why split pump from recv
//!
//! A naive `tokio::select!` between `mpsc::Receiver::recv` and
//! the V8 event loop inside a single task
//! starves whichever arm is unlucky. Under sustained 64-connection
//! load, the receive arm wins almost every iteration, V8 microtasks
//! never run, and tail latency collapses (`p99` jumps from
//! single-digit ms to ~70 ms in the Docker `2cpu-1024mb` preset).
//!
//! Splitting the worker into **two cooperating `spawn_local` tasks**
//! sharing one [`Rc<RefCell<V8Engine>>`] eliminates the bias:
//!
//! 1. **Pump task** - drives V8 by polling the event loop in a
//!    [`std::future::poll_fn`]. The mutable borrow is reacquired per
//!    poll and dropped on the synchronous return path so the recv
//!    task can borrow between V8 ticks. When V8 reports the loop
//!    drained the pump parks on a [`tokio::sync::Notify`] until the
//!    next dispatch.
//! 2. **Recv task** - pulls jobs off the public mailbox, calls
//!    [`V8Engine::enqueue`] (a `&self` API), spawns a per-request
//!    forwarder that awaits the JS reply, then notifies the pump.
//!
//! The Tokio `current_thread` scheduler interleaves the two tasks
//! fairly because each has its own poll point - Axum acceptors,
//! forwarders, the pump, and the recv loop all rotate.
//!
//! ## Module scope
//!
//! Helpers in this module are `pub(super)`-only and assume they are
//! called inside an active [`tokio::task::LocalSet`]. Each worker
//! variant supplies its own supervisor loop that owns the
//! `Rc<RefCell<V8Engine>>` and decides what to do on shutdown
//! (multi-thread workers exit; single-thread workers may rebuild
//! in-place).

#![allow(
    clippy::future_not_send,
    reason = "all helpers run on a !Send LocalSet and intentionally hold \
              Rc<RefCell<V8Engine>> across .await points"
)]

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use std::sync::OnceLock;

use tokio::sync::Notify;

use super::pump_strategy::pump_strategy_from_env;
use super::worker::{DispatchJob, WorkerError, WorkerHealth};
use crate::engine::cjs::{FsResolver, ROOT_PARENT};
use crate::engine::{BootContext, CodeCache, IsolateHandle, V8Engine};
use crate::ops::{OsEnv, ProcessConfig, RequestMeta, RequestSlot};
use crate::sandbox_root_for;

/// Process-wide V8 bytecode cache shared across every worker isolate.
///
/// Lazily built from environment on first reference. Keeping a single
/// instance means counters aggregate across workers and on-disk
/// writes share the same atomic-rename rendezvous.
fn process_code_cache() -> CodeCache {
    static CACHE: OnceLock<CodeCache> = OnceLock::new();
    CACHE.get_or_init(CodeCache::from_env).clone()
}

/// Boots a fresh [`V8Engine`], starts the JS pump matching the
/// configured strategy, and wraps the engine for shared
/// pump/recv access on a single [`tokio::task::LocalSet`].
///
/// `mode_label` is included in the boot tracing line so operators can
/// tell ST and MT logs apart at a glance.
///
/// # Errors
///
/// [`WorkerError::Engine`] when the engine fails to boot or when the
/// JS pump installer reports a setup error.
pub(super) async fn boot_engine(
    entrypoint: &Path,
    mode_label: &'static str,
    worker_id: crate::ops::WorkerId,
    workers: usize,
) -> Result<Rc<RefCell<V8Engine>>, WorkerError> {
    let mut engine = {
        let registry = crate::engine::cjs::default_registry()
            .map_err(|err| WorkerError::Engine(err.to_string()))?;
        let registry = Arc::new(registry);
        let project_root = sandbox_root_for(entrypoint);
        let resolver = Arc::new(FsResolver::new(vec![project_root.clone()], registry));
        let ctx = BootContext::new()
            .with_cjs(resolver)
            .with_cjs_root(ROOT_PARENT)
            .with_worker_id(worker_id)
            .with_fs(crate::ops::FsHandle::real(vec![project_root]))
            .with_process(ProcessConfig::builder(Arc::new(OsEnv)).build())
            .with_code_cache(process_code_cache());
        V8Engine::boot_with(entrypoint, ctx)
            .await
            .map_err(|err| WorkerError::Engine(err.to_string()))?
    };

    let strategy = pump_strategy_from_env(std::env::var("NEXIDE_PUMP_BATCH").ok().as_deref())
        .unwrap_or_else(|| super::pump_strategy::default_pump_strategy_for(workers));
    let batch_cap: usize = if strategy.name() == "coalesced" {
        strategy.max_inflight_per_tick() as usize
    } else {
        0
    };
    tracing::debug!(
        strategy = strategy.name(),
        batch_cap,
        mode = mode_label,
        worker = worker_id.id,
        "nexide-worker: starting JS pump"
    );
    engine
        .start_pump(batch_cap)
        .map_err(|err| WorkerError::Engine(err.to_string()))?;

    Ok(Rc::new(RefCell::new(engine)))
}

/// Drives [`super::V8Engine::pump_once`] one tick at a
/// time. Yields the mutable borrow between polls so the recv task can
/// enqueue new slots, and parks on `pump_signal` once V8 reports the
/// event loop drained - avoids spinning on an idle isolate.
///
/// While parked the pump arms a single-shot `idle GC` timer
/// configured by [`idle_gc_threshold`]: if no request arrives within
/// the threshold the worker calls
/// [`V8Engine::notify_low_memory`](super::V8Engine::notify_low_memory)
/// (and on Linux/jemalloc also nudges the allocator to return dirty
/// pages to the OS via [`purge_jemalloc_arenas`]). The timer rearms
/// only after the next request - so we pay *at most one* major GC per
/// idle period regardless of how long the silence lasts. The default
/// (`30_000` ms) keeps the GC out of the hot path while letting an
/// idle worker shed `~80-120` MiB of reclaimable heap before the
/// container scaler ever observes the pressure.
///
/// Returns when V8 reports an event-loop error (the supervisor is
/// expected to recycle/rebuild the worker in that case) or the task
/// is cancelled.
pub(super) async fn run_pump(engine: Rc<RefCell<V8Engine>>, pump_signal: Rc<Notify>) {
    let napi_wakeup = engine.borrow().napi_wakeup();
    let idle_threshold = idle_gc_threshold();
    let mut idle_gc_armed = true;
    loop {
        engine.borrow_mut().pump_once();
        let queue_empty = {
            let e = engine.borrow();
            e.queue_is_empty()
        };
        if queue_empty {
            let woken_by_request = wait_for_work(&pump_signal, &napi_wakeup, idle_threshold).await;
            if woken_by_request {
                idle_gc_armed = true;
            } else if idle_gc_armed {
                run_idle_reclaim(&engine);
                idle_gc_armed = false;
            }
        } else {
            tokio::task::yield_now().await;
            idle_gc_armed = true;
        }
    }
}

/// Awaits either a pump signal (new request enqueued) or a NAPI
/// wakeup, optionally with an idle-GC deadline. Returns `true` when a
/// real wakeup arrived, `false` when the deadline elapsed (caller
/// should run the idle-reclaim path).
async fn wait_for_work(
    pump_signal: &Notify,
    napi_wakeup: &Notify,
    idle_threshold: Option<std::time::Duration>,
) -> bool {
    match idle_threshold {
        Some(d) => {
            let work = async {
                tokio::select! {
                    () = pump_signal.notified() => {},
                    () = napi_wakeup.notified() => {},
                }
            };
            tokio::time::timeout(d, work).await.is_ok()
        }
        None => {
            tokio::select! {
                () = pump_signal.notified() => {},
                () = napi_wakeup.notified() => {},
            }
            true
        }
    }
}

/// Asks V8 to free reclaimable heap. Called at most once per idle
/// period from the pump task.
///
/// We intentionally do not poke jemalloc / glibc directly here: the
/// `jemalloc` feature already configures aggressive decay
/// (`dirty_decay_ms` / `muzzy_decay_ms`), which returns pages to the
/// OS within a few seconds of being freed without any explicit
/// `purge` call. The V8 notification triggers a major GC that
/// *frees* the pages in the first place, so the allocator decay can
/// follow up naturally - one notification, two layers of reclaim.
fn run_idle_reclaim(engine: &Rc<RefCell<V8Engine>>) {
    let before = engine.borrow().heap_stats();
    engine.borrow_mut().notify_low_memory();
    let after = engine.borrow().heap_stats();
    let cache = process_code_cache();
    let evicted = if cache.is_enabled() {
        cache.evict_to_quota()
    } else {
        0
    };
    let snap = cache.metrics().snapshot();
    let shrunk = super::idle_shrink::shrink_all();
    tracing::debug!(
        heap_before = before.used_heap_size,
        heap_after = after.used_heap_size,
        reclaimed = before.used_heap_size.saturating_sub(after.used_heap_size),
        cache_hits = snap.hits,
        cache_misses = snap.misses,
        cache_rejects = snap.rejects,
        cache_writes = snap.writes,
        cache_evicted = evicted,
        ram_shrinkers = shrunk,
        "idle reclaim: V8 low-memory notification"
    );
}

/// Resolves the idle-GC threshold from `NEXIDE_IDLE_GC_MS`.
///
/// `0` (or any unparseable value) disables the idle-GC path entirely
/// for operators who would rather spend RSS on a hot path that needs
/// it. Default `30_000` ms - long enough that the GC pause never
/// lands on a real request after the bench harness's 2-second
/// cooldown, short enough that idle deployments shed memory before
/// the autoscaler reads RSS.
fn idle_gc_threshold() -> Option<std::time::Duration> {
    parse_idle_gc_threshold(std::env::var("NEXIDE_IDLE_GC_MS").ok().as_deref())
}

/// Pure parser for the `NEXIDE_IDLE_GC_MS` env value, exposed for
/// unit tests so we can pin the precedence rules without poking
/// process-global state.
fn parse_idle_gc_threshold(raw: Option<&str>) -> Option<std::time::Duration> {
    let trimmed = raw.map(str::trim).filter(|s| !s.is_empty());
    let ms: u64 = trimmed.and_then(|s| s.parse().ok()).unwrap_or(30_000);
    if ms == 0 {
        None
    } else {
        Some(std::time::Duration::from_millis(ms))
    }
}

/// Registers `job` with the engine by handing the dispatcher's reply
/// oneshot directly to the in-isolate
/// [`crate::ops::DispatchTable`]. The JS handler completes the
/// dispatcher's await via `op_nexide_send_response` /
/// `op_nexide_finish_error` - there is no intermediate forwarder
/// task and no per-request oneshot allocation on this path.
///
/// `handled` is incremented eagerly (one bump per submitted job).
/// `RequestCount`-based recycle policies tolerate this - completed
/// vs. submitted differ at most by the per-isolate concurrency cap,
/// which is bounded.
///
/// Slot construction errors short-circuit the dispatcher with
/// [`WorkerError::BadRequest`] without touching the V8 isolate.
pub(super) fn register_job(
    engine: &V8Engine,
    job: DispatchJob,
    health: &Arc<Mutex<WorkerHealth>>,
    handled: &Arc<AtomicU64>,
) {
    let slot = match build_slot(job.request) {
        Ok(slot) => slot,
        Err(err) => {
            let _ = job
                .reply
                .send(Err(crate::ops::RequestFailure::Handler(err.to_string())));
            return;
        }
    };
    let _ = engine.enqueue_with(slot, job.reply);
    let count = handled.fetch_add(1, Ordering::Relaxed) + 1;
    let heap = engine.heap_stats();
    if let Ok(mut snapshot) = health.lock() {
        *snapshot = WorkerHealth {
            heap,
            requests_handled: count,
        };
    }
}

/// Validates a [`crate::dispatch::ProtoRequest`] and converts it into
/// the engine-facing [`RequestSlot`].
///
/// Method/URI parsing is the only fallible step; bodies and headers
/// are passed through unchanged.
///
/// # Errors
///
/// [`WorkerError::BadRequest`] when the method or URI fails parsing.
pub(super) fn build_slot(
    request: crate::dispatch::ProtoRequest,
) -> Result<RequestSlot, WorkerError> {
    let meta = RequestMeta::try_new(request.method, request.uri)
        .map_err(|err| WorkerError::BadRequest(err.to_string()))?;
    Ok(RequestSlot::new(meta, request.headers, request.body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_slot_rejects_invalid_method() {
        let bad = crate::dispatch::ProtoRequest {
            method: "BAD METHOD".to_owned(),
            uri: "/".to_owned(),
            headers: Vec::new(),
            body: bytes::Bytes::new(),
        };
        let result = build_slot(bad);
        assert!(matches!(result, Err(WorkerError::BadRequest(_))));
    }

    #[test]
    fn build_slot_accepts_valid_request() {
        let good = crate::dispatch::ProtoRequest {
            method: "GET".to_owned(),
            uri: "/".to_owned(),
            headers: Vec::new(),
            body: bytes::Bytes::new(),
        };
        let result = build_slot(good);
        assert!(result.is_ok());
    }

    #[test]
    fn idle_gc_threshold_defaults_to_thirty_seconds_when_unset_or_blank() {
        assert_eq!(
            parse_idle_gc_threshold(None),
            Some(std::time::Duration::from_millis(30_000))
        );
        assert_eq!(
            parse_idle_gc_threshold(Some("")),
            Some(std::time::Duration::from_millis(30_000))
        );
        assert_eq!(
            parse_idle_gc_threshold(Some("   ")),
            Some(std::time::Duration::from_millis(30_000))
        );
    }

    #[test]
    fn idle_gc_threshold_zero_disables_path() {
        assert_eq!(parse_idle_gc_threshold(Some("0")), None);
    }

    #[test]
    fn idle_gc_threshold_parses_explicit_millis() {
        assert_eq!(
            parse_idle_gc_threshold(Some("1500")),
            Some(std::time::Duration::from_millis(1500))
        );
        assert_eq!(
            parse_idle_gc_threshold(Some("  60000  ")),
            Some(std::time::Duration::from_millis(60_000))
        );
    }

    #[test]
    fn idle_gc_threshold_falls_back_to_default_when_unparseable() {
        assert_eq!(
            parse_idle_gc_threshold(Some("not-a-number")),
            Some(std::time::Duration::from_millis(30_000))
        );
        assert_eq!(
            parse_idle_gc_threshold(Some("-5")),
            Some(std::time::Duration::from_millis(30_000))
        );
    }
}
