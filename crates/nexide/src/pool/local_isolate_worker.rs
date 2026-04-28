//! Single-threaded [`Worker`] implementation that runs the V8 isolate
//! on the **caller's** Tokio `LocalSet` instead of dedicating a fresh
//! OS thread.
//!
//! ## Why this exists (single-thread mode)
//!
//! [`super::IsolateWorker`] solves the "isolates are `!Send`" problem
//! by giving every worker its own OS thread plus a dedicated
//! `current_thread` runtime. That works beautifully on multi-CPU
//! deployments — each isolate gets its own core — but on a `--cpus=1`
//! Docker container it adds a 60 µs/request cross-thread futex tax
//! (see `docs/PERF_NOTES.md` Iteration 2).
//!
//! [`LocalIsolateWorker`] removes that tax by running the V8 isolate
//! as a `tokio::task::spawn_local` task on the **same** runtime that
//! Axum is accepting connections on. The per-job `mpsc` channel still
//! exists (so the public [`Worker`] handle stays `Send + Sync`), but
//! in single-thread mode both endpoints are polled on the same OS
//! thread and the underlying mpsc reduces to an intra-runtime task
//! wake — no syscall, no context switch.
//!
//! ## Two-task pump (Iteracja 4 — fair scheduling)
//!
//! A naive `tokio::select!` on `recv()` vs `run_event_loop()` inside
//! a single task **starves Axum** when V8 has plenty of microtasks to
//! drain: the biased select keeps re-firing the V8 arm, the recv arm
//! never blocks long enough to let the `LocalSet` round-robin to other
//! tasks (Axum acceptors, per-request forwarders), and 64-connection
//! `/api/*` workloads collapse with 100% client-side timeouts.
//!
//! The fix: split the worker into **two `spawn_local` tasks** sharing
//! a single [`Rc<RefCell<V8Engine>>`].
//!
//! 1. **Pump task** — drives V8 by calling
//!    [`super::V8Engine::pump_once`] inside a
//!    [`std::future::poll_fn`]. The mutable borrow is created **per
//!    poll** and dropped on the synchronous return path, so the recv
//!    task can safely re-borrow between V8 polls. When V8 reports the
//!    event loop drained (`Ready(Ok(()))`), the pump waits on a
//!    [`tokio::sync::Notify`] until the next [`Self::dispatch`] call
//!    enqueues fresh work, instead of busy-polling an already-idle
//!    isolate.
//! 2. **Recv task** — pulls jobs off the public mailbox, calls
//!    [`V8Engine::enqueue`] (a `&self` API), spawns a per-request
//!    forwarder for the JS reply, and notifies the pump.
//!
//! Tokio's `current_thread` scheduler is cooperative, but each task
//! has its own poll point, so it can fairly interleave Axum accept,
//! V8 ticks, and forwarder replies. Axum acceptors are no longer
//! starved by the worker's hot loop.
//!
//! The receiver task **must** be spawned inside an active
//! [`tokio::task::LocalSet`]; the public constructor enforces this by
//! calling [`tokio::task::spawn_local`] directly. Callers therefore
//! invoke [`LocalIsolateWorker::spawn_local`] from inside
//! `LocalSet::block_on(&rt, async { ... })` (see
//! `crate::run_single_thread`).

#![allow(
    clippy::future_not_send,
    clippy::significant_drop_tightening,
    reason = "this module intentionally hosts !Send futures on a LocalSet \
              and binds the V8 isolate to a single Rc<RefCell<...>> across \
              boot, pump, and recv tasks"
)]

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::{Notify, mpsc, oneshot};

use super::engine_pump::{boot_engine as shared_boot_engine, register_job, run_pump};
use super::worker::{DispatchJob, Job, Worker, WorkerError, WorkerHealth};
use crate::engine::V8Engine;
use crate::ops::ResponsePayload;

/// Mailbox capacity (matches the multi-thread variant's bound).
///
/// The `Sender` side never blocks under steady-state load because the
/// receiver task is co-resident on the same thread and is woken
/// immediately after `send`; the bound exists purely as a back-pressure
/// circuit-breaker for pathological burst scenarios.
const MAILBOX_CAPACITY: usize = 256;

/// `Send + Sync` proxy to a V8 isolate hosted on a Tokio `LocalSet`.
pub struct LocalIsolateWorker {
    job_tx: mpsc::Sender<DispatchJob>,
    rebuild_tx: mpsc::Sender<oneshot::Sender<Result<(), WorkerError>>>,
    health: Arc<Mutex<WorkerHealth>>,
}

impl LocalIsolateWorker {
    /// Boots the V8 isolate on the **current** `LocalSet`, wires up
    /// the JS pump and returns a `Send + Sync` handle ready to
    /// dispatch.
    ///
    /// # Errors
    ///
    /// [`WorkerError::Engine`] when the engine fails to boot;
    /// [`WorkerError::Shutdown`] when the boot future is dropped before
    /// the readiness signal fires.
    ///
    /// # Panics
    ///
    /// Panics (via [`tokio::task::spawn_local`]) when called outside
    /// an active [`tokio::task::LocalSet`]. This is by contract — the
    /// type only makes sense inside a `current_thread` runtime that
    /// also drives Axum.
    pub async fn spawn_local(
        entrypoint: PathBuf,
        worker_id: usize,
        workers: usize,
    ) -> Result<Self, WorkerError> {
        let (job_tx, job_rx) = mpsc::channel::<DispatchJob>(MAILBOX_CAPACITY);
        let (rebuild_tx, rebuild_rx) = mpsc::channel::<oneshot::Sender<Result<(), WorkerError>>>(1);
        let (ready_tx, ready_rx) = oneshot::channel::<Result<(), WorkerError>>();
        let health = Arc::new(Mutex::new(WorkerHealth::fresh()));
        let health_for_task = Arc::clone(&health);

        tokio::task::spawn_local(async move {
            run_local_worker(
                entrypoint,
                worker_id,
                workers,
                job_rx,
                rebuild_rx,
                ready_tx,
                health_for_task,
            )
            .await;
        });

        ready_rx.await.map_err(|_| WorkerError::Shutdown)??;

        Ok(Self {
            job_tx,
            rebuild_tx,
            health,
        })
    }

    /// Asks the supervisor to drop the current V8 isolate and boot a
    /// fresh one **on the same `LocalSet` thread**, then awaits the
    /// rebuild outcome.
    ///
    /// Single-thread mode invariant: only one V8 isolate may be
    /// "entered" on a given thread at a time. The recycler must not
    /// build a second isolate alongside the live one — it instead
    /// signals the existing supervisor, which tears down the old
    /// isolate before booting the new one.
    ///
    /// In-flight handlers are aborted via the engine's `fail_inflight`
    /// path; their dispatchers observe a `WorkerError::Engine` reply.
    ///
    /// # Errors
    ///
    /// [`WorkerError::Shutdown`] when the supervisor task is gone or
    /// the channel is full (back-pressure: another rebuild is already
    /// queued); [`WorkerError::Engine`] when the engine fails to boot.
    pub async fn rebuild(&self) -> Result<(), WorkerError> {
        let (ack_tx, ack_rx) = oneshot::channel();
        self.rebuild_tx
            .try_send(ack_tx)
            .map_err(|_| WorkerError::Shutdown)?;
        ack_rx.await.map_err(|_| WorkerError::Shutdown)?
    }
}

#[async_trait]
impl Worker for LocalIsolateWorker {
    async fn dispatch(&self, job: Job) -> Result<ResponsePayload, WorkerError> {
        let (reply_tx, reply_rx) = oneshot::channel::<crate::ops::CompletionResult>();
        self.job_tx
            .send(DispatchJob {
                request: job.request,
                reply: reply_tx,
            })
            .await
            .map_err(|_| WorkerError::Shutdown)?;
        match reply_rx.await {
            Ok(Ok(payload)) => Ok(payload),
            Ok(Err(failure)) => Err(WorkerError::Engine(failure.to_string())),
            Err(_) => Err(WorkerError::Shutdown),
        }
    }

    fn health(&self) -> WorkerHealth {
        *self.health.lock().expect("health mutex poisoned")
    }
}

/// Top-level supervisor task that owns the isolate, kicks off the
/// pump and recv subtasks, and arranges in-flight failure on shutdown.
///
/// When a rebuild request arrives over `rebuild_rx`, the pump task is
/// aborted, in-flight slots are failed, the engine is dropped (V8
/// isolate teardown happens here — synchronous, no other isolate can
/// be active during this window), and the loop boots a fresh engine.
/// The supervisor lives until `job_rx` closes.
async fn run_local_worker(
    entrypoint: PathBuf,
    worker_id: usize,
    workers: usize,
    mut job_rx: mpsc::Receiver<DispatchJob>,
    mut rebuild_rx: mpsc::Receiver<oneshot::Sender<Result<(), WorkerError>>>,
    ready_tx: oneshot::Sender<Result<(), WorkerError>>,
    health: Arc<Mutex<WorkerHealth>>,
) {
    let engine = match boot_engine(&entrypoint, worker_id, workers).await {
        Ok(engine) => engine,
        Err(err) => {
            let _ = ready_tx.send(Err(err));
            return;
        }
    };
    let _ = ready_tx.send(Ok(()));

    let handled = Arc::new(AtomicU64::new(0));
    let mut current_engine = engine;

    loop {
        let outcome = drive_engine_until_event(
            current_engine,
            &mut job_rx,
            &mut rebuild_rx,
            &health,
            &handled,
        )
        .await;

        match outcome {
            DriveOutcome::ChannelClosed(engine) => {
                let engine_ref = engine.borrow();
                engine_ref.fail_inflight("channel closed");
                return;
            }
            DriveOutcome::RebuildRequested { engine, ack } => {
                engine.borrow().fail_inflight("worker rebuild");
                drop(engine);

                match boot_engine(&entrypoint, worker_id, workers).await {
                    Ok(fresh_engine) => {
                        current_engine = fresh_engine;
                        if let Ok(mut snapshot) = health.lock() {
                            *snapshot = WorkerHealth::fresh();
                        }
                        handled.store(0, Ordering::Relaxed);
                        let _ = ack.send(Ok(()));
                        tracing::info!("nexide-worker (local): isolate rebuilt");
                    }
                    Err(err) => {
                        tracing::error!(
                            error = %err,
                            "nexide-worker (local): rebuild failed — terminating worker"
                        );
                        let _ = ack.send(Err(err));
                        return;
                    }
                }
            }
        }
    }
}

/// Outcome of a single "drive the engine" pass — either the public
/// mailbox closed (terminal) or a rebuild was requested.
enum DriveOutcome {
    ChannelClosed(Rc<RefCell<V8Engine>>),
    RebuildRequested {
        engine: Rc<RefCell<V8Engine>>,
        ack: oneshot::Sender<Result<(), WorkerError>>,
    },
}

/// Spawns the pump for `engine`, runs the recv loop until the public
/// mailbox closes or a rebuild is requested, then aborts the pump and
/// returns the engine handle (still the only `Rc` live on the thread).
async fn drive_engine_until_event(
    engine: Rc<RefCell<V8Engine>>,
    job_rx: &mut mpsc::Receiver<DispatchJob>,
    rebuild_rx: &mut mpsc::Receiver<oneshot::Sender<Result<(), WorkerError>>>,
    health: &Arc<Mutex<WorkerHealth>>,
    handled: &Arc<AtomicU64>,
) -> DriveOutcome {
    let pump_signal = Rc::new(Notify::new());
    let pump_engine = Rc::clone(&engine);
    let pump_signal_for_pump = Rc::clone(&pump_signal);
    let pump_handle =
        tokio::task::spawn_local(async move { run_pump(pump_engine, pump_signal_for_pump).await });

    let event = run_recv_loop(&engine, job_rx, rebuild_rx, &pump_signal, health, handled).await;

    pump_handle.abort();
    let _ = pump_handle.await;

    match event {
        RecvEvent::ChannelClosed => DriveOutcome::ChannelClosed(engine),
        RecvEvent::Rebuild(ack) => DriveOutcome::RebuildRequested { engine, ack },
    }
}

/// Boots a fresh [`V8Engine`], starts its JS pump and wraps it for
/// shared access between the supervisor's pump and recv tasks.
async fn boot_engine(
    entrypoint: &Path,
    worker_id: usize,
    workers: usize,
) -> Result<Rc<RefCell<V8Engine>>, WorkerError> {
    let id = crate::ops::WorkerId::new(worker_id, worker_id == 0);
    shared_boot_engine(entrypoint, "single-thread", id, workers).await
}

/// What caused the recv loop to return.
enum RecvEvent {
    ChannelClosed,
    Rebuild(oneshot::Sender<Result<(), WorkerError>>),
}

/// Reads jobs from the public mailbox and registers each with the
/// engine. Notifies the pump so it can advance V8 if it was idle.
/// Also services rebuild requests by returning early to the supervisor.
async fn run_recv_loop(
    engine: &Rc<RefCell<V8Engine>>,
    job_rx: &mut mpsc::Receiver<DispatchJob>,
    rebuild_rx: &mut mpsc::Receiver<oneshot::Sender<Result<(), WorkerError>>>,
    pump_signal: &Rc<Notify>,
    health: &Arc<Mutex<WorkerHealth>>,
    handled: &Arc<AtomicU64>,
) -> RecvEvent {
    loop {
        tokio::select! {
            biased;
            ack = rebuild_rx.recv() => match ack {
                Some(ack) => return RecvEvent::Rebuild(ack),
                None => return RecvEvent::ChannelClosed,
            },
            job = job_rx.recv() => match job {
                Some(job) => {
                    let engine_ref = engine.borrow();
                    register_job(&engine_ref, job, health, handled);
                    drop(engine_ref);
                    pump_signal.notify_one();
                }
                None => return RecvEvent::ChannelClosed,
            },
        }
    }
}

/// Drives [`super::V8Engine::pump_once`] one tick at a
/// time. Yields the borrow between polls so the recv task can enqueue
/// new slots, and parks on `pump_signal` once V8 reports the event
/// loop drained — avoids spinning on an idle isolate.
///
/// Implementation lives in [`super::engine_pump::run_pump`].
///
/// Registers `job` with the engine using
/// [`super::engine_pump::register_job`] (no per-request task spawn,
/// caller's reply oneshot is handed to V8 directly).
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
        let result = super::super::engine_pump::build_slot(bad);
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
        let result = super::super::engine_pump::build_slot(good);
        assert!(result.is_ok());
    }
}
