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

use tokio::sync::Notify;

use super::pump_strategy::pump_strategy_from_env;
use super::worker::{DispatchJob, WorkerError, WorkerHealth};
use crate::engine::cjs::{FsResolver, ROOT_PARENT};
use crate::engine::{BootContext, IsolateHandle, V8Engine};
use crate::ops::{RequestMeta, RequestSlot};
use crate::sandbox_root_for;

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
        let resolver = Arc::new(FsResolver::new(vec![project_root], registry));
        let ctx = BootContext::new()
            .with_cjs(resolver)
            .with_cjs_root(ROOT_PARENT)
            .with_worker_id(worker_id);
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
/// Returns when V8 reports an event-loop error (the supervisor is
/// expected to recycle/rebuild the worker in that case) or the task
/// is cancelled.
pub(super) async fn run_pump(engine: Rc<RefCell<V8Engine>>, pump_signal: Rc<Notify>) {
    let napi_wakeup = engine.borrow().napi_wakeup();
    loop {
        engine.borrow_mut().pump_once();
        let queue_empty = {
            let e = engine.borrow();
            e.queue_is_empty()
        };
        if queue_empty {
            tokio::select! {
                () = pump_signal.notified() => {},
                () = napi_wakeup.notified() => {},
            }
        } else {
            tokio::task::yield_now().await;
        }
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
}
