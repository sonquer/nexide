//! Accept loop that distributes incoming TCP connections across the
//! per-worker [`super::worker_runtime::WorkerRuntime`] mailboxes.
//!
//! ## Architecture
//!
//! Replaces both `axum::serve(TcpListener, ..)` and the historical
//! `IsolatePool::dispatch` per-request mpsc hop with a **single**
//! cross-thread hop per HTTP connection (not per request). Once a
//! connection lands on a worker every subsequent request on that
//! keep-alive connection is processed end-to-end on the worker's
//! thread - Axum, prerender, and the V8 isolate all share the same
//! `current_thread` runtime, so request dispatch is intra-thread
//! (direct task wake-up, no syscall).
//!
//! ## Picker
//!
//! Adaptive *power of two choices* over the workers' mailbox queue
//! depth (see [`WorkerRuntime::queue_depth`]). Two atomic counters
//! with coprime strides (1 and 7) sample two candidate workers; the
//! one with the lower queue depth wins. Under uniform load the picker
//! degenerates to plain round-robin (deterministic for tests). When
//! the chosen worker's mailbox is full the loop falls back to the
//! second candidate; if both are full the connection is dropped with
//! a `warn!` rather than blocked, preserving the acceptor's
//! responsiveness under sustained overload.
//!
//! ## Shutdown
//!
//! The loop terminates when the supplied `shutdown` future resolves.
//! It does **not** drain in-flight connections - that is the workers'
//! job, driven by their own watch-channel shutdown signal in
//! [`super::worker_runtime`].

use std::future::Future;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use thiserror::Error;
use tokio::net::TcpListener;

use super::worker_runtime::WorkerRuntime;

/// Stride applied to the secondary p2c counter. Coprime to the
/// primary stride (`1`) so the two picker indices walk independent
/// orbits around the worker ring; matches the existing
/// `IsolatePool` picker rationale.
const PICKER_STRIDE_B: usize = 7;

/// Errors raised by [`run_accept_loop`].
#[derive(Debug, Error)]
pub enum AcceptError {
    /// Fatal `accept(2)` error. The acceptor logs and retries on
    /// transient errors; this variant wraps cases the OS reports as
    /// permanent (e.g. listener socket closed).
    #[error("accept loop: fatal accept error: {0}")]
    Fatal(#[source] io::Error),
}

/// Runs the accept loop until `shutdown` resolves.
///
/// `workers` must be non-empty - empty pools are a misconfiguration
/// the caller is expected to surface earlier.
///
/// # Errors
///
/// See [`AcceptError`].
///
/// # Panics
///
/// Panics if `workers` is empty (programming error).
pub async fn run_accept_loop<F>(
    listener: TcpListener,
    workers: Arc<[WorkerRuntime]>,
    shutdown: F,
) -> Result<(), AcceptError>
where
    F: Future<Output = ()> + Send,
{
    assert!(
        !workers.is_empty(),
        "accept loop requires at least one worker"
    );
    let picker_a = AtomicUsize::new(0);
    let picker_b = AtomicUsize::new(0);
    tokio::pin!(shutdown);

    loop {
        let accept = listener.accept();
        tokio::select! {
            biased;
            () = &mut shutdown => {
                tracing::info!("accept loop: shutdown signal received");
                return Ok(());
            }
            outcome = accept => {
                match outcome {
                    Ok((stream, addr)) => {
                        if let Err(err) = stream.set_nodelay(true) {
                            tracing::debug!(error = %err, "accept loop: failed to set TCP_NODELAY");
                        }
                        dispatch_stream(&workers, &picker_a, &picker_b, stream, addr);
                    }
                    Err(err) => {
                        if is_transient_accept_error(&err) {
                            tracing::warn!(error = %err, "accept loop: transient accept error");
                            tokio::task::yield_now().await;
                            continue;
                        }
                        return Err(AcceptError::Fatal(err));
                    }
                }
            }
        }
    }
}

/// Picks a worker via p2c and forwards `stream` to it.
///
/// Falls back to the alternate p2c candidate when the primary's
/// mailbox is full. If both are full the connection is dropped with
/// a `warn!` - the alternative (blocking the accept loop on `send`)
/// would let one slow worker stall the entire process.
fn dispatch_stream(
    workers: &[WorkerRuntime],
    picker_a: &AtomicUsize,
    picker_b: &AtomicUsize,
    stream: tokio::net::TcpStream,
    addr: std::net::SocketAddr,
) {
    let (primary, secondary) = pick_worker(workers, picker_a, picker_b);
    match workers[primary].try_send_stream(stream, addr) {
        Ok(()) => {}
        Err(tokio::sync::mpsc::error::TrySendError::Full((stream, addr))) => {
            if let Err(err) = workers[secondary].try_send_stream(stream, addr) {
                tracing::warn!(
                    primary,
                    secondary,
                    error = ?err,
                    "accept loop: both p2c candidates full - dropping connection"
                );
            }
        }
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
            tracing::error!(
                worker = primary,
                "accept loop: worker mailbox closed - connection dropped"
            );
        }
    }
}

/// Selects two distinct candidate worker indices.
///
/// The primary index advances by stride `1` (round-robin baseline),
/// the secondary by stride `PICKER_STRIDE_B`. When both samples land
/// on the same slot the secondary is rotated by 1. Under contended
/// load the caller compares queue depths and forwards to the lower
/// candidate; under uniform load the function reduces to round-robin
/// (the primary always wins ties at the call site).
fn pick_worker(
    workers: &[WorkerRuntime],
    picker_a: &AtomicUsize,
    picker_b: &AtomicUsize,
) -> (usize, usize) {
    let n = workers.len();
    let a = picker_a.fetch_add(1, Ordering::Relaxed) % n;
    let mut b = (picker_b.fetch_add(PICKER_STRIDE_B, Ordering::Relaxed)) % n;
    if b == a {
        b = (b + 1) % n;
    }
    if n == 1 {
        return (0, 0);
    }
    let depth_a = workers[a].queue_depth();
    let depth_b = workers[b].queue_depth();
    if depth_b < depth_a { (b, a) } else { (a, b) }
}

/// Returns `true` for `accept(2)` errors that the loop can retry
/// after a yield. Mirrors the strategy used by `axum::serve`'s
/// internal helper of the same name (see `axum/src/serve/listener.rs`).
fn is_transient_accept_error(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::Interrupted
            | io::ErrorKind::WouldBlock
    )
}

#[cfg(test)]
mod tests {
    use super::is_transient_accept_error;
    use std::io;

    #[test]
    fn transient_errors_are_retryable() {
        for kind in [
            io::ErrorKind::ConnectionAborted,
            io::ErrorKind::ConnectionReset,
            io::ErrorKind::Interrupted,
            io::ErrorKind::WouldBlock,
        ] {
            let err = io::Error::new(kind, "x");
            assert!(is_transient_accept_error(&err), "{kind:?}");
        }
    }

    #[test]
    fn other_errors_are_fatal() {
        let err = io::Error::new(io::ErrorKind::AddrInUse, "x");
        assert!(!is_transient_accept_error(&err));
    }
}
