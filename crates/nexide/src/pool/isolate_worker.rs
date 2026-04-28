//! Production [`Worker`] implementation backed by [`V8Engine`] for
//! multi-thread (`--cpus>1`) deployments.
//!
//! V8 isolates are `!Send`, so each `IsolateWorker` owns a dedicated
//! OS thread with its own `current_thread` Tokio runtime and
//! `LocalSet`. The struct itself is a `Send + Sync` proxy that
//! forwards requests over an `mpsc` channel - `dispatch` therefore
//! takes `&self` and is safe to call concurrently from many tasks.
//!
//! ## Two-task pump
//!
//! The naive `tokio::select!` between `mpsc::Receiver::recv` and
//! the V8 event loop starves V8 microtasks under sustained load:
//! under wrk -c64 the recv arm wins almost every iteration, queued
//! JS callbacks never fire, and `/api/*` tail latency collapses
//! (Docker `2cpu-1024mb`: `p99` 67 ms before the fix, single-digit
//! ms after).
//!
//! The classic two-future event-loop driver pattern makes the
//! same point - both futures must be polled on every wake, never
//! biased towards one. We get the same fairness for free by
//! splitting the worker into two `tokio::task::spawn_local` tasks
//! sharing one [`Rc<RefCell<V8Engine>>`]:
//!
//! 1. **Pump task** - drives V8 through
//!    [`super::V8Engine::pump_once`] and parks on a
//!    [`tokio::sync::Notify`] when idle.
//! 2. **Recv task** - pulls jobs off the public mailbox, calls
//!    [`V8Engine::enqueue`] (`&self`), spawns a per-request
//!    forwarder, then notifies the pump.
//!
//! Both tasks live on the worker thread's `LocalSet`, so the
//! `current_thread` scheduler interleaves them fairly. The shared
//! implementation lives in [`super::engine_pump`].
//!

#![allow(
    clippy::future_not_send,
    clippy::significant_drop_tightening,
    reason = "this module hosts the V8 isolate on a dedicated worker \
              thread; pump and recv tasks share Rc<RefCell<V8Engine>>"
)]

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::{Notify, mpsc, oneshot};

use super::engine_pump::{boot_engine, register_job, run_pump};
use super::worker::{DispatchJob, Job, Worker, WorkerError, WorkerHealth};

/// Capacity of the per-worker mailbox. Sized generously so bursts of
/// concurrent requests can pipeline into V8 without blocking the
/// dispatch path on `Sender::send().await`.
const MAILBOX_CAPACITY: usize = 256;

/// Tokio current-thread runtime polling intervals.
///
/// Tighter intervals trade a small amount of throughput on idle
/// isolates for shorter scheduling tails under burst load. The
/// values match the previous runtime's per-runtime defaults but make
/// the choice explicit in case upstream drifts.
const EVENT_INTERVAL: u32 = 31;
const GLOBAL_QUEUE_INTERVAL: u32 = 31;

/// Send-safe handle to a single V8 isolate running on a dedicated
/// worker thread.
pub struct IsolateWorker {
    job_tx: mpsc::Sender<DispatchJob>,
    health: Arc<Mutex<WorkerHealth>>,
}

impl IsolateWorker {
    /// Spawns the worker thread, boots a [`V8Engine`] from
    /// `entrypoint`, and returns a handle ready to dispatch.
    ///
    /// # Errors
    ///
    /// [`WorkerError::Engine`] if the worker thread cannot be spawned
    /// or the engine fails to boot.
    pub async fn spawn(
        entrypoint: PathBuf,
        worker_id: usize,
        workers: usize,
    ) -> Result<Self, WorkerError> {
        let (job_tx, job_rx) = mpsc::channel::<DispatchJob>(MAILBOX_CAPACITY);
        let (ready_tx, ready_rx) = oneshot::channel::<Result<(), WorkerError>>();
        let health = Arc::new(Mutex::new(WorkerHealth::fresh()));
        let health_for_thread = health.clone();

        std::thread::Builder::new()
            .name(format!("nexide-worker-{worker_id}"))
            .spawn(move || {
                worker_thread_main(
                    entrypoint,
                    worker_id,
                    workers,
                    job_rx,
                    ready_tx,
                    health_for_thread,
                );
            })
            .map_err(|err| WorkerError::Engine(format!("spawn failed: {err}")))?;

        ready_rx.await.map_err(|_| WorkerError::Shutdown)??;

        Ok(Self { job_tx, health })
    }
}

#[async_trait]
impl Worker for IsolateWorker {
    async fn dispatch(&self, job: Job) -> Result<crate::ops::ResponsePayload, WorkerError> {
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

fn worker_thread_main(
    entrypoint: PathBuf,
    worker_id: usize,
    workers: usize,
    job_rx: mpsc::Receiver<DispatchJob>,
    ready_tx: oneshot::Sender<Result<(), WorkerError>>,
    health: Arc<Mutex<WorkerHealth>>,
) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .event_interval(EVENT_INTERVAL)
        .global_queue_interval(GLOBAL_QUEUE_INTERVAL)
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            let _ = ready_tx.send(Err(WorkerError::Engine(format!("rt build: {err}"))));
            return;
        }
    };
    let local = tokio::task::LocalSet::new();
    local.block_on(
        &rt,
        run_worker_loop(entrypoint, worker_id, workers, job_rx, ready_tx, health),
    );
}

/// Worker thread main loop: boots the engine, signals readiness,
/// then runs the pump and recv tasks until the public mailbox closes
/// or the pump fails.
///
/// On shutdown all in-flight requests are aborted via
/// [`crate::engine::V8Engine::fail_inflight`] so dispatchers
/// observe a [`WorkerError::Engine`] reply rather than a hung
/// oneshot.
async fn run_worker_loop(
    entrypoint: PathBuf,
    worker_id: usize,
    workers: usize,
    mut job_rx: mpsc::Receiver<DispatchJob>,
    ready_tx: oneshot::Sender<Result<(), WorkerError>>,
    health: Arc<Mutex<WorkerHealth>>,
) {
    let id = crate::ops::WorkerId::new(worker_id, worker_id == 0);
    let engine = match boot_engine(&entrypoint, "multi-thread", id, workers).await {
        Ok(engine) => {
            let _ = ready_tx.send(Ok(()));
            engine
        }
        Err(err) => {
            let _ = ready_tx.send(Err(err));
            return;
        }
    };

    let handled = Arc::new(AtomicU64::new(0));
    let pump_signal = Rc::new(Notify::new());
    let pump_engine = Rc::clone(&engine);
    let pump_signal_for_pump = Rc::clone(&pump_signal);
    let pump_handle = tokio::task::spawn_local(async move {
        run_pump(pump_engine, pump_signal_for_pump).await;
    });

    let shutdown_reason: &str =
        run_recv_loop(&engine, &mut job_rx, &pump_signal, &health, &handled).await;

    pump_handle.abort();
    let _ = pump_handle.await;
    engine.borrow().fail_inflight(shutdown_reason);
}

/// Pulls jobs from the public mailbox, registers them with the
/// engine, and notifies the pump task to advance V8 if it was idle.
///
/// Returns when the mailbox closes - the supervisor uses the reason
/// string to fail in-flight handlers.
async fn run_recv_loop(
    engine: &Rc<std::cell::RefCell<crate::engine::V8Engine>>,
    job_rx: &mut mpsc::Receiver<DispatchJob>,
    pump_signal: &Rc<Notify>,
    health: &Arc<Mutex<WorkerHealth>>,
    handled: &Arc<AtomicU64>,
) -> &'static str {
    while let Some(job) = job_rx.recv().await {
        let engine_ref = engine.borrow();
        register_job(&engine_ref, job, health, handled);
        drop(engine_ref);
        pump_signal.notify_one();
    }
    "channel closed"
}
