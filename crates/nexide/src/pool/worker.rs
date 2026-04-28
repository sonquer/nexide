//! Worker abstraction and `Send`-safe handle types used by
//! [`super::IsolatePool`].

use async_trait::async_trait;
use thiserror::Error;
use tokio::sync::oneshot;

use crate::dispatch::ProtoRequest;
use crate::engine::HeapStats;
use crate::ops::{CompletionResult, ResponsePayload};

/// Snapshot of a worker's runtime state used by the pool to decide
/// whether to recycle it.
///
/// Cheap to copy (`Copy`) so the pool can read it without holding any
/// lock for the duration of the recycle policy evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerHealth {
    /// Last observed V8 heap statistics.
    pub heap: HeapStats,
    /// Number of requests handled since this worker was instantiated.
    pub requests_handled: u64,
}

impl WorkerHealth {
    /// Returns the canonical "just spawned" snapshot (zero requests,
    /// empty heap stats). Cheaper than calling `Default::default()`.
    #[must_use]
    pub const fn fresh() -> Self {
        Self {
            heap: HeapStats {
                used_heap_size: 0,
                total_heap_size: 0,
                heap_size_limit: 0,
            },
            requests_handled: 0,
        }
    }
}

impl Default for WorkerHealth {
    fn default() -> Self {
        Self::fresh()
    }
}

/// Failure modes raised by [`Worker::dispatch`].
#[derive(Debug, Error)]
pub enum WorkerError {
    /// The worker thread is no longer running.
    #[error("worker: shutdown")]
    Shutdown,

    /// A request meta validation error from the dispatch layer.
    #[error("worker: invalid request: {0}")]
    BadRequest(String),

    /// The underlying engine reported an error.
    #[error("worker: engine error: {0}")]
    Engine(String),
}

/// Single unit of work dispatched to a [`Worker`].
#[derive(Debug)]
pub struct Job {
    /// The HTTP request to be served.
    pub request: ProtoRequest,
}

/// Behavioural contract every pool worker must implement.
///
/// Workers are `Send + Sync + 'static` so the pool can hand out
/// concurrent `&self` references to many in-flight dispatches without
/// any per-slot lock. The underlying V8 isolate is single-threaded by
/// construction, so concrete implementations are expected to forward
/// requests over an internal channel that natively supports
/// multi-producer / single-consumer semantics (e.g. `mpsc::Sender`).
#[async_trait]
pub trait Worker: Send + Sync + 'static {
    /// Hands `job` to the worker and waits for the assembled response.
    ///
    /// Implementations must be safe to call concurrently from multiple
    /// tasks — the pool relies on lock-free dispatch to pipeline
    /// requests into a worker's mailbox while V8 drains it serially.
    ///
    /// # Errors
    ///
    /// See [`WorkerError`].
    async fn dispatch(&self, job: Job) -> Result<ResponsePayload, WorkerError>;

    /// Latest health snapshot (Query — pure, no side effects).
    fn health(&self) -> WorkerHealth;
}

/// Internal job format used by the channel between `IsolateWorker`
/// (Send proxy) and its dedicated thread (which owns the engine).
///
/// `reply` is the **same** [`oneshot::Sender`] handed off to the V8
/// [`crate::ops::DispatchTable`] — the JS handler completes the
/// dispatcher's await directly via `op_nexide_send_response` /
/// `op_nexide_finish_error`. Eliminating the per-request forwarder
/// task removes one [`tokio::task::spawn_local`] and one
/// `oneshot::channel` allocation from the hot path; see the
/// performance notes in `docs/PERF_NOTES.md`.
pub(super) struct DispatchJob {
    pub(super) request: ProtoRequest,
    pub(super) reply: oneshot::Sender<CompletionResult>,
}
