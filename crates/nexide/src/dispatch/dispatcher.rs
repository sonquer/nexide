//! Trait + concrete implementation of the cross-thread dispatcher.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use bytes::Bytes;
use tokio::sync::{mpsc, oneshot};

use super::errors::DispatchError;
use crate::engine::cjs::{FsResolver, ROOT_PARENT, default_registry};
use crate::engine::{BootContext, V8Engine};
use crate::ops::{HeaderPair, OsEnv, ProcessConfig, RequestMeta, RequestSlot, ResponsePayload};
use crate::sandbox_root_for;

/// Plain-data view of an HTTP request used to cross thread boundaries.
///
/// The Axum side converts `Request<Body>` into [`ProtoRequest`] before
/// posting it to the worker; the worker hands it to [`V8Engine`] as
/// a [`RequestSlot`].
#[derive(Debug)]
pub struct ProtoRequest {
    /// Request method (already validated).
    pub method: String,
    /// Request-target as it appeared on the wire.
    pub uri: String,
    /// Request headers in lowercased form.
    pub headers: Vec<HeaderPair>,
    /// Request body, fully buffered.
    pub body: Bytes,
}

impl ProtoRequest {
    fn into_slot(self) -> Result<RequestSlot, DispatchError> {
        let meta =
            RequestMeta::try_new(self.method, self.uri).map_err(DispatchError::BadRequest)?;
        Ok(RequestSlot::new(meta, self.headers, self.body))
    }
}

/// Cross-thread dispatcher contract used by the HTTP shield.
///
/// Implementors must be `Send + Sync + 'static` so Axum can clone the
/// handle for every incoming connection. The engine itself is `!Send`;
/// concrete implementors solve the conflict by owning a channel
/// endpoint that talks to a single dedicated thread.
#[async_trait]
pub trait EngineDispatcher: Send + Sync + 'static {
    /// Hands `request` to the JS handler and waits for the assembled
    /// response.
    ///
    /// # Errors
    ///
    /// See [`DispatchError`] - propagates worker, engine and response
    /// failures to the caller (Axum maps them to HTTP `502` /
    /// `504`).
    async fn dispatch(&self, request: ProtoRequest) -> Result<ResponsePayload, DispatchError>;

    /// Total number of requests dispatched since the worker started
    /// (Query - telemetry only, no side effects).
    fn dispatch_count(&self) -> usize;
}

/// Job submitted to the worker thread.
struct DispatchJob {
    slot: RequestSlot,
    reply: oneshot::Sender<Result<ResponsePayload, DispatchError>>,
}

/// Production [`EngineDispatcher`] backed by a dedicated thread that
/// owns one [`V8Engine`].
///
/// Construction boots the engine before returning; subsequent
/// [`Self::dispatch`] calls are zero-startup-overhead.
pub struct IsolateDispatcher {
    job_tx: mpsc::Sender<DispatchJob>,
    dispatch_count: Arc<AtomicUsize>,
}

impl IsolateDispatcher {
    /// Spawns the worker thread, boots the engine pointed to by
    /// `entrypoint`, and returns a handle ready to dispatch.
    ///
    /// Awaits engine bootstrap so callers observe boot failures
    /// immediately rather than at first request.
    ///
    /// # Errors
    ///
    /// [`DispatchError::Engine`] when the worker fails to boot;
    /// [`DispatchError::WorkerGone`] when the worker thread cannot be
    /// spawned at all.
    pub async fn spawn(entrypoint: PathBuf) -> Result<Self, DispatchError> {
        let (job_tx, job_rx) = mpsc::channel::<DispatchJob>(64);
        let (ready_tx, ready_rx) = oneshot::channel::<Result<(), DispatchError>>();

        std::thread::Builder::new()
            .name("nexide-isolate".to_owned())
            .spawn(move || worker_main(entrypoint, job_rx, ready_tx))
            .map_err(|err| DispatchError::BodyRead(format!("spawn failed: {err}")))?;

        ready_rx.await.map_err(|_| DispatchError::WorkerGone)??;

        Ok(Self {
            job_tx,
            dispatch_count: Arc::new(AtomicUsize::new(0)),
        })
    }
}

#[async_trait]
impl EngineDispatcher for IsolateDispatcher {
    async fn dispatch(&self, request: ProtoRequest) -> Result<ResponsePayload, DispatchError> {
        let slot = request.into_slot()?;
        let (reply_tx, reply_rx) = oneshot::channel();
        self.job_tx
            .send(DispatchJob {
                slot,
                reply: reply_tx,
            })
            .await
            .map_err(|_| DispatchError::WorkerGone)?;
        let outcome = reply_rx.await.map_err(|_| DispatchError::WorkerGone)?;
        if outcome.is_ok() {
            self.dispatch_count.fetch_add(1, Ordering::Relaxed);
        }
        outcome
    }

    fn dispatch_count(&self) -> usize {
        self.dispatch_count.load(Ordering::Relaxed)
    }
}

fn worker_main(
    entrypoint: PathBuf,
    mut job_rx: mpsc::Receiver<DispatchJob>,
    ready_tx: oneshot::Sender<Result<(), DispatchError>>,
) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            let _ = ready_tx.send(Err(DispatchError::BodyRead(format!("rt build: {err}"))));
            return;
        }
    };
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, run_worker(entrypoint, &mut job_rx, ready_tx));
}

#[allow(clippy::future_not_send, clippy::significant_drop_tightening)]
async fn run_worker(
    entrypoint: PathBuf,
    job_rx: &mut mpsc::Receiver<DispatchJob>,
    ready_tx: oneshot::Sender<Result<(), DispatchError>>,
) {
    let registry = match default_registry() {
        Ok(reg) => Arc::new(reg),
        Err(err) => {
            let _ = ready_tx.send(Err(DispatchError::BodyRead(format!(
                "registry build: {err}"
            ))));
            return;
        }
    };
    let project_root = sandbox_root_for(&entrypoint);
    let resolver = Arc::new(FsResolver::new(vec![project_root.clone()], registry));
    let ctx = BootContext::new()
        .with_cjs(resolver)
        .with_cjs_root(ROOT_PARENT)
        .with_fs(crate::ops::FsHandle::real(vec![project_root]))
        .with_process(ProcessConfig::builder(Arc::new(OsEnv)).build());

    let mut engine = match V8Engine::boot_with(&entrypoint, ctx).await {
        Ok(engine) => {
            let _ = ready_tx.send(Ok(()));
            engine
        }
        Err(err) => {
            let _ = ready_tx.send(Err(DispatchError::Engine(err)));
            return;
        }
    };
    if let Err(err) = engine.start_pump(0) {
        tracing::error!(error = %err, "nexide-dispatcher: failed to start JS request pump");
        return;
    }

    let engine = std::rc::Rc::new(std::cell::RefCell::new(engine));
    let pump_signal = std::rc::Rc::new(tokio::sync::Notify::new());
    let pump_engine = engine.clone();
    let pump_handle = {
        let signal = pump_signal.clone();
        tokio::task::spawn_local(async move {
            loop {
                pump_engine.borrow_mut().pump_once();
                let queue_empty = pump_engine.borrow().queue_is_empty();
                if queue_empty {
                    signal.notified().await;
                } else {
                    tokio::task::yield_now().await;
                }
            }
        })
    };

    let shutdown_reason: &str = loop {
        let Some(job) = job_rx.recv().await else {
            break "channel closed";
        };
        let rx = engine.borrow().enqueue(job.slot);
        pump_signal.notify_one();
        let pump_signal_for_task = pump_signal.clone();
        tokio::task::spawn_local(async move {
            let result = match rx.await {
                Ok(Ok(payload)) => Ok(payload),
                Ok(Err(failure)) => Err(DispatchError::BodyRead(failure.to_string())),
                Err(_) => Err(DispatchError::WorkerGone),
            };
            pump_signal_for_task.notify_one();
            let _ = job.reply.send(result);
        });
    };

    pump_handle.abort();
    engine.borrow().fail_inflight(shutdown_reason);
}
