//! Process-wide queue of OS-delivered signals waiting to be observed
//! by the JavaScript `process` event emitter.
//!
//! ## Why a queue
//!
//! V8 isolates run on dedicated `LocalSet`s; we can't synchronously
//! invoke JS handlers from a Tokio signal task without crossing the
//! `!Send` boundary. The bridge instead pushes signal *names*
//! ("SIGTERM", "SIGINT", …) into a shared mailbox; the
//! `process` polyfill polls it from a 100 ms `setInterval` and emits
//! the corresponding events on its [`EventEmitter`]. Latency is
//! bounded by the polling cadence (well under any sensible
//! `terminationGracePeriodSeconds` on Kubernetes).
//!
//! ## Hands-off use
//!
//! [`bind_termination_signals`] arms `tokio::signal::unix` for
//! SIGTERM, SIGINT, and SIGHUP and pushes the canonical names. The
//! returned future resolves the **first** time SIGTERM or SIGINT
//! arrives so the embedder can flip its graceful-shutdown watch
//! channel - SIGHUP is observed but does not by itself trigger
//! shutdown (Node doesn't shut down on SIGHUP either; long-running
//! services use it for reload-style notifications).

use std::sync::Mutex;
use std::sync::OnceLock;

/// Process-wide FIFO of signal names yet to be drained by the JS
/// polyfill. Each entry is a `'static` Node-style identifier
/// (`SIGTERM`, `SIGINT`, …); the `&'static str` choice keeps the
/// allocation footprint independent of the queue depth.
fn queue() -> &'static Mutex<Vec<&'static str>> {
    static Q: OnceLock<Mutex<Vec<&'static str>>> = OnceLock::new();
    Q.get_or_init(|| Mutex::new(Vec::new()))
}

/// Records that signal `name` was delivered to the host process.
///
/// Safe to call from a Tokio signal task or any other thread; the
/// queue uses a mutex with poison-tolerant access (poisoned guard
/// recovered through `into_inner`) so a panicking JS poll cannot
/// permanently silence the bridge.
pub fn push(name: &'static str) {
    let mut g = queue()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    g.push(name);
}

/// Drains the queue, returning every signal observed since the
/// previous call (in arrival order).
#[must_use]
pub fn drain() -> Vec<&'static str> {
    let mut g = queue()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    std::mem::take(&mut *g)
}

/// Listens for SIGTERM / SIGINT / SIGHUP on Unix (or Ctrl-C on other
/// platforms) and pushes the canonical signal name onto the queue.
///
/// Resolves the first time SIGTERM or SIGINT is observed - the caller
/// is expected to flip the runtime's shutdown watch channel after the
/// future returns. SIGHUP is recorded but doesn't trigger shutdown.
#[cfg(unix)]
pub async fn bind_termination_signals() {
    use tokio::signal::unix::{SignalKind, signal};

    let mut sigterm = match signal(SignalKind::terminate()) {
        Ok(s) => s,
        Err(err) => {
            tracing::error!(%err, "failed to install SIGTERM handler");
            return;
        }
    };
    let mut sigint = match signal(SignalKind::interrupt()) {
        Ok(s) => s,
        Err(err) => {
            tracing::error!(%err, "failed to install SIGINT handler");
            return;
        }
    };
    let mut sighup = match signal(SignalKind::hangup()) {
        Ok(s) => s,
        Err(err) => {
            tracing::warn!(%err, "failed to install SIGHUP handler");
            // SIGHUP is non-essential; keep waiting for SIGTERM/SIGINT.
            tokio::select! {
                _ = sigterm.recv() => {
                    push("SIGTERM");
                }
                _ = sigint.recv() => {
                    push("SIGINT");
                }
            }
            return;
        }
    };
    loop {
        tokio::select! {
            _ = sigterm.recv() => {
                push("SIGTERM");
                return;
            }
            _ = sigint.recv() => {
                push("SIGINT");
                return;
            }
            _ = sighup.recv() => {
                push("SIGHUP");
                // SIGHUP doesn't end the wait - keep listening so a
                // subsequent SIGTERM still triggers shutdown.
            }
        }
    }
}

/// Non-Unix fallback: only Ctrl-C (SIGINT-equivalent) is observed.
#[cfg(not(unix))]
pub async fn bind_termination_signals() {
    if let Err(err) = tokio::signal::ctrl_c().await {
        tracing::error!(%err, "failed to listen for ctrl-c");
        return;
    }
    push("SIGINT");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drain_returns_pushed_signals_in_fifo_order() {
        // Drain anything queued by other tests first.
        let _ = drain();
        push("SIGTERM");
        push("SIGINT");
        let drained = drain();
        assert_eq!(drained, vec!["SIGTERM", "SIGINT"]);
        assert!(drain().is_empty(), "queue should be empty after drain");
    }
}
