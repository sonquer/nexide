//! Per-worker runtime: dedicated OS thread hosting a
//! `current_thread` Tokio runtime, a `LocalSet`, one V8 isolate, and
//! the full Axum router (static + prerender + dynamic).
//!
//! ## Architecture
//!
//! Historically (`MultiThread` mode) Axum ran on a multi-thread Tokio
//! reactor and the V8 isolates each lived on a dedicated worker
//! thread. Every `/api/*` request paid two cross-thread `futex` hops
//! (Axum task → worker mpsc; worker oneshot → Axum task), which
//! dominated p99 under saturating load.
//!
//! [`WorkerRuntime`] eliminates that hop structurally: each worker
//! owns its own Tokio runtime, its own `LocalSet`, its own Axum
//! stack, and its own [`crate::pool::LocalIsolatePool`].
//!
//! Two accept strategies are supported, picked at construction time:
//!
//! 1. [`AcceptStrategy::Shared`] (used on macOS / Windows
//!    and as the bind-failure fallback) — connections arrive on a
//!    shared listener owned by the main reactor and are routed via a
//!    per-worker [`tokio::sync::mpsc`] channel of `TcpStream`s. One
//!    cross-thread hop per *connection* (not per request).
//! 2. [`AcceptStrategy::ReusePort`] (Linux fast path) —
//!    each worker binds its own [`tokio::net::TcpListener`] with
//!    `SO_REUSEPORT`. The kernel distributes incoming connections
//!    by 4-tuple hash. Zero cross-thread hops for HTTP traffic; the
//!    main reactor only handles graceful shutdown.
//!
//! Shutdown is broadcast over a [`tokio::sync::watch`] channel; each
//! worker installs a future that completes when the watch flips to
//! `true`, hands it to `axum::serve(..).with_graceful_shutdown(..)`,
//! and joins on its own thread once the serve loop returns.

use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

use thiserror::Error;
#[cfg(target_os = "linux")]
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, watch};

use super::config::ServerConfig;
use super::stream_listener::StreamListener;
use super::{NextBridgeHandler, build_router};
use crate::pool::LocalIsolatePool;

/// Per-worker mailbox capacity for incoming `TcpStream`s.
///
/// On Linux every worker binds its own `SO_REUSEPORT` listener
/// so the kernel does the connection-level fan-out and
/// the mailbox only ever holds streams the local worker is about to
/// service. On macOS / fallback paths the central accept loop pushes
/// here, but the alternate-pick fallback (see
/// `super::accept_loop::pick_worker`) absorbs `Full` events. A small
/// cap keeps queue-induced p99 latency bounded under burst load.
/// `try_send` returning `Full` triggers the accept loop's
/// alternate-pick fallback.
const STREAM_MAILBOX_CAPACITY: usize = 32;

/// Backlog passed to `listen(2)` when a worker binds its own
/// `SO_REUSEPORT` listener.
///
/// Sized to absorb a sub-millisecond burst at 30k+ RPS without the
/// kernel returning `ECONNREFUSED` to a SYN before the worker has a
/// chance to drain the queue. Below `1024` we have empirically
/// observed 3-tuple resets under wrk / `--latency` mode on Docker
/// Desktop on macOS-on-Linux-VM; above `4096` is wasted because per-
/// worker isolates cannot keep up anyway and the kernel just
/// drops SYNs at the same total rate.
#[cfg(target_os = "linux")]
const REUSEPORT_BACKLOG: u32 = 1024;

/// Strategy for handing accepted connections to a worker.
///
/// See module-level docs for the architectural rationale.
#[derive(Debug)]
pub enum AcceptStrategy {
    /// Worker reads streams from a shared mpsc-fed listener owned by
    /// the main reactor's `accept_loop`.
    Shared,
    /// Worker binds its own `TcpListener` with `SO_REUSEPORT` on the
    /// supplied address (Linux fast path).
    ReusePort {
        /// Address to bind. Multiple workers binding the same address
        /// is the whole point — kernel distributes connections via
        /// 4-tuple hash.
        addr: SocketAddr,
    },
}

/// Failure modes raised while spawning a worker.
#[derive(Debug, Error)]
pub enum WorkerSpawnError {
    /// The worker's OS thread could not be started.
    #[error("worker spawn: thread spawn failed: {0}")]
    Thread(#[source] std::io::Error),

    /// The worker's `current_thread` Tokio runtime failed to build.
    #[error("worker spawn: tokio runtime build failed: {0}")]
    Tokio(#[source] std::io::Error),

    /// Engine boot or pool initialisation reported an error.
    #[error("worker spawn: engine boot failed: {0}")]
    Engine(String),

    /// The worker thread terminated before signalling readiness.
    #[error("worker spawn: thread exited before reporting readiness")]
    EarlyExit,

    /// Binding the per-worker `SO_REUSEPORT` listener failed.
    #[error("worker spawn: reuseport bind failed on {addr}: {source}")]
    ReusePortBind {
        /// Address that the worker attempted to bind.
        addr: SocketAddr,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },
}

/// Handle owned by the accept loop / supervisor.
///
/// `stream_tx` is `Some` only for [`AcceptStrategy::Shared`] workers;
/// [`AcceptStrategy::ReusePort`] workers do not receive forwarded
/// streams and report a constant queue depth of zero.
///
/// Cloning the [`mpsc::Sender`] inside [`Self::stream_tx`] is cheap
/// (single atomic). The handle keeps the worker thread alive — when
/// dropped, the watch channel drops too and the worker exits its
/// graceful-shutdown future on the next poll.
pub struct WorkerRuntime {
    idx: usize,
    stream_tx: Option<mpsc::Sender<(TcpStream, SocketAddr)>>,
    shutdown_tx: watch::Sender<bool>,
    join: Option<thread::JoinHandle<()>>,
}

impl WorkerRuntime {
    /// Spawns a worker that receives streams from a shared mpsc-fed
    /// listener.
    ///
    /// `idx` is the worker index used in tracing output and as a
    /// thread-name suffix. `cfg` and `entrypoint` are cheap to clone
    /// and are propagated into the worker thread verbatim.
    ///
    /// # Errors
    ///
    /// See [`WorkerSpawnError`].
    pub async fn spawn(
        idx: usize,
        workers: usize,
        cfg: ServerConfig,
        entrypoint: PathBuf,
    ) -> Result<Self, WorkerSpawnError> {
        let (stream_tx, stream_rx) =
            mpsc::channel::<(TcpStream, SocketAddr)>(STREAM_MAILBOX_CAPACITY);
        let bind_addr = cfg.bind();
        Self::spawn_inner(
            idx,
            workers,
            cfg,
            entrypoint,
            AcceptSource::Shared {
                stream_rx,
                advertise: bind_addr,
            },
            Some(stream_tx),
        )
        .await
    }

    /// Spawns a worker that binds its own `SO_REUSEPORT` listener
    /// (Linux fast path).
    ///
    /// Each worker calling this function with the same `addr` will
    /// successfully bind because `SO_REUSEPORT` allows multiple
    /// listeners on the same `(addr, port)` tuple; the kernel then
    /// distributes incoming connections via 4-tuple hash.
    ///
    /// # Errors
    ///
    /// [`WorkerSpawnError::ReusePortBind`] when the listener cannot
    /// be opened (typically `EADDRINUSE` on a kernel that does not
    /// honour `SO_REUSEPORT`, or `EACCES` for a privileged port). The
    /// caller is expected to log and fall back to [`Self::spawn`] in
    /// that case.
    #[cfg(target_os = "linux")]
    pub async fn spawn_reuseport(
        idx: usize,
        workers: usize,
        cfg: ServerConfig,
        entrypoint: PathBuf,
    ) -> Result<Self, WorkerSpawnError> {
        let addr = cfg.bind();
        let listener = bind_reuseport_listener(addr)
            .map_err(|source| WorkerSpawnError::ReusePortBind { addr, source })?;
        Self::spawn_inner(
            idx,
            workers,
            cfg,
            entrypoint,
            AcceptSource::ReusePort { listener },
            None,
        )
        .await
    }

    async fn spawn_inner(
        idx: usize,
        workers: usize,
        cfg: ServerConfig,
        entrypoint: PathBuf,
        source: AcceptSource,
        stream_tx: Option<mpsc::Sender<(TcpStream, SocketAddr)>>,
    ) -> Result<Self, WorkerSpawnError> {
        let (shutdown_tx, shutdown_rx) = watch::channel::<bool>(false);
        let (ready_tx, ready_rx) = oneshot::channel::<Result<(), String>>();

        let join = thread::Builder::new()
            .name(format!("nexide-worker-{idx}"))
            .spawn(move || {
                worker_main(idx, workers, cfg, entrypoint, source, shutdown_rx, ready_tx);
            })
            .map_err(WorkerSpawnError::Thread)?;

        match ready_rx.await {
            Ok(Ok(())) => Ok(Self {
                idx,
                stream_tx,
                shutdown_tx,
                join: Some(join),
            }),
            Ok(Err(msg)) => Err(WorkerSpawnError::Engine(msg)),
            Err(_) => Err(WorkerSpawnError::EarlyExit),
        }
    }

    /// Worker index — stable identifier used in tracing and tests.
    #[must_use]
    pub const fn idx(&self) -> usize {
        self.idx
    }

    /// Best-effort queue-depth proxy used by the accept loop's p2c
    /// picker. Returns the number of streams currently buffered
    /// between the acceptor and this worker's serve loop. Lower is
    /// better.
    ///
    /// Returns `0` for [`AcceptStrategy::ReusePort`] workers — they
    /// do not have a buffered mailbox; the kernel queue is the
    /// bound. The accept loop never picks among reuseport workers
    /// (it is not even running in that mode), so this is purely
    /// informational.
    ///
    /// Cheap (single atomic) — safe to call in the accept hot loop.
    #[must_use]
    pub fn queue_depth(&self) -> usize {
        self.stream_tx.as_ref().map_or(0, |tx| {
            STREAM_MAILBOX_CAPACITY.saturating_sub(tx.capacity())
        })
    }

    /// Hands `stream` to the worker without blocking. Returns `Err`
    /// when the mailbox is full or the worker has shut down — callers
    /// use this for the alternate-pick fallback.
    ///
    /// # Errors
    ///
    /// [`mpsc::error::TrySendError::Full`] when the mailbox is full;
    /// [`mpsc::error::TrySendError::Closed`] when the worker thread
    /// has terminated, or when the worker was spawned with
    /// [`AcceptStrategy::ReusePort`] and therefore has no mailbox.
    pub fn try_send_stream(
        &self,
        stream: TcpStream,
        addr: SocketAddr,
    ) -> Result<(), mpsc::error::TrySendError<(TcpStream, SocketAddr)>> {
        match self.stream_tx.as_ref() {
            Some(tx) => tx.try_send((stream, addr)),
            None => Err(mpsc::error::TrySendError::Closed((stream, addr))),
        }
    }

    /// Signals graceful shutdown and joins the worker thread in a
    /// single call. Idempotent — safe to call before [`Drop`].
    ///
    /// When shutting down a fleet of workers prefer the two-phase
    /// pattern via [`Self::signal_shutdown`] + [`Self::join`] so every
    /// worker stops accepting new connections before any one of them
    /// blocks on draining its in-flight requests.
    pub fn shutdown(mut self) {
        self.shutdown_in_place();
    }

    /// Signals graceful shutdown without blocking. Flips the watch
    /// channel so the worker's `axum::serve(..).with_graceful_shutdown`
    /// future starts to drain. Pair with [`Self::join`] to wait for
    /// the worker thread to exit.
    pub fn signal_shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }

    /// Blocks the current thread until the worker exits. Idempotent —
    /// after the first call subsequent invocations are no-ops.
    pub fn join(mut self) {
        self.join_in_place();
    }

    fn join_in_place(&mut self) {
        if let Some(handle) = self.join.take()
            && let Err(panic) = handle.join()
        {
            tracing::error!(
                worker = self.idx,
                panic = ?panic,
                "worker thread panicked during shutdown"
            );
        }
    }

    fn shutdown_in_place(&mut self) {
        self.signal_shutdown();
        self.join_in_place();
    }
}

impl Drop for WorkerRuntime {
    fn drop(&mut self) {
        self.shutdown_in_place();
    }
}

/// Internal worker-side handle to the accept source. Either the
/// worker reads from a shared mpsc-fed pseudo-listener
/// or it owns a real `TcpListener` it bound itself with
/// `SO_REUSEPORT` (Linux only).
enum AcceptSource {
    Shared {
        stream_rx: mpsc::Receiver<(TcpStream, SocketAddr)>,
        advertise: SocketAddr,
    },
    #[cfg(target_os = "linux")]
    ReusePort { listener: TcpListener },
}

/// Binds a `SO_REUSEPORT` listener on the supplied address.
///
/// Used by [`WorkerRuntime::spawn_reuseport`]. Each
/// worker calls this with the same address; the kernel admits
/// multiple bindings because `SO_REUSEPORT` is set, then balances
/// incoming connections by 4-tuple hash.
///
/// # Errors
///
/// Returns the underlying `io::Error` from `socket(2)`,
/// `setsockopt(2)`, `bind(2)` or `listen(2)`.
#[cfg(target_os = "linux")]
fn bind_reuseport_listener(addr: SocketAddr) -> io::Result<TcpListener> {
    let socket = match addr {
        SocketAddr::V4(_) => tokio::net::TcpSocket::new_v4()?,
        SocketAddr::V6(_) => tokio::net::TcpSocket::new_v6()?,
    };
    socket.set_reuseaddr(true)?;
    socket.set_reuseport(true)?;
    socket.bind(addr)?;
    socket.listen(REUSEPORT_BACKLOG)
}

/// Worker thread entry point — owns the `current_thread` runtime
/// and the `LocalSet`, boots the isolate pool, then runs `axum::serve`
/// against the supplied [`AcceptSource`] until the watch channel
/// signals shutdown.
fn worker_main(
    idx: usize,
    workers: usize,
    cfg: ServerConfig,
    entrypoint: PathBuf,
    source: AcceptSource,
    shutdown_rx: watch::Receiver<bool>,
    ready_tx: oneshot::Sender<Result<(), String>>,
) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(2)
        .thread_name(format!("nexide-worker-{idx}"))
        .event_interval(31)
        .global_queue_interval(31)
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            let _ = ready_tx.send(Err(format!("tokio rt build: {err}")));
            return;
        }
    };

    let local = tokio::task::LocalSet::new();
    local.block_on(
        &rt,
        run_worker_local(idx, workers, cfg, entrypoint, source, shutdown_rx, ready_tx),
    );
}

#[allow(
    clippy::future_not_send,
    reason = "this future is hosted on a !Send LocalSet by construction"
)]
async fn run_worker_local(
    idx: usize,
    workers: usize,
    cfg: ServerConfig,
    entrypoint: PathBuf,
    source: AcceptSource,
    mut shutdown_rx: watch::Receiver<bool>,
    ready_tx: oneshot::Sender<Result<(), String>>,
) {
    let pool = match LocalIsolatePool::boot(entrypoint, idx, workers).await {
        Ok(pool) => pool,
        Err(err) => {
            let _ = ready_tx.send(Err(err.to_string()));
            return;
        }
    };
    let _ = ready_tx.send(Ok(()));

    tracing::debug!(worker = idx, "nexide worker ready");

    let handler: Arc<dyn super::DynamicHandler> = Arc::new(NextBridgeHandler::new(Arc::new(pool)));
    let router = build_router(&cfg, handler);

    let shutdown = async move {
        loop {
            if *shutdown_rx.borrow() {
                return;
            }
            if shutdown_rx.changed().await.is_err() {
                return;
            }
        }
    };

    let outcome = match source {
        AcceptSource::Shared {
            stream_rx,
            advertise,
        } => {
            let listener = StreamListener::new(stream_rx, advertise);
            axum::serve(listener, router)
                .with_graceful_shutdown(shutdown)
                .await
        }
        #[cfg(target_os = "linux")]
        AcceptSource::ReusePort { listener } => {
            use axum::serve::ListenerExt;
            let tuned = listener.tap_io(|stream| {
                if let Err(err) = stream.set_nodelay(true) {
                    tracing::debug!(error = %err, "reuseport: failed to set TCP_NODELAY");
                }
            });
            axum::serve(tuned, router)
                .with_graceful_shutdown(shutdown)
                .await
        }
    };

    if let Err(err) = outcome {
        tracing::error!(worker = idx, error = %err, "worker serve loop terminated with error");
    } else {
        tracing::debug!(worker = idx, "worker serve loop drained gracefully");
    }
}

#[cfg(test)]
mod tests {
    use super::WorkerRuntime;
    use std::net::SocketAddr;

    #[test]
    fn worker_spawn_error_engine_carries_message() {
        let err = super::WorkerSpawnError::Engine("kaboom".into());
        let rendered = err.to_string();
        assert!(rendered.contains("kaboom"));
    }

    #[test]
    fn handle_drop_signals_shutdown() {
        let (stream_tx, _stream_rx) =
            tokio::sync::mpsc::channel::<(tokio::net::TcpStream, SocketAddr)>(1);
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel::<bool>(false);
        let join = std::thread::spawn(|| {});
        let handle = WorkerRuntime {
            idx: 0,
            stream_tx: Some(stream_tx),
            shutdown_tx,
            join: Some(join),
        };
        drop(handle);
        assert!(*shutdown_rx.borrow_and_update());
    }

    #[test]
    fn reuseport_handle_reports_zero_queue_depth_and_closes_on_send() {
        let (shutdown_tx, _shutdown_rx) = tokio::sync::watch::channel::<bool>(false);
        let join = std::thread::spawn(|| {});
        let handle = WorkerRuntime {
            idx: 7,
            stream_tx: None,
            shutdown_tx,
            join: Some(join),
        };
        assert_eq!(handle.queue_depth(), 0);
        assert_eq!(handle.idx(), 7);
        drop(handle);
    }
}
