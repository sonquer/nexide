//! Custom [`axum::serve::Listener`] backed by an in-process channel
//! of accepted [`TcpStream`]s.
//!
//! ## Why this exists
//!
//! `axum::serve` is married to a `Listener` trait that does its own
//! `accept`. To eliminate the cross-thread hop between Axum and the
//! `!Send` V8 isolate we host **one Axum stack per worker thread** -
//! each on its own `current_thread` Tokio runtime + `LocalSet`. A
//! single shared TCP listener cannot be bound multiple times on the
//! same port without `SO_REUSEPORT`, which (a) is platform-dependent
//! and (b) would replace our adaptive in-process load balance with a
//! 4-tuple kernel hash.
//!
//! Instead, an *accept loop* on the multi-thread reactor accepts real
//! TCP connections, picks a worker (p2c by mailbox depth), and pushes
//! the stream through a [`tokio::sync::mpsc::Sender`] to the worker.
//! [`StreamListener`] is the worker-side endpoint that turns those
//! pushes into the [`axum::serve::Listener`] interface so the rest of
//! the existing serve loop (router + graceful shutdown) keeps working
//! verbatim.

use std::io;
use std::net::SocketAddr;

use tokio::net::TcpStream;
use tokio::sync::mpsc;

/// Per-worker mailbox endpoint exposed as an [`axum::serve::Listener`].
pub(super) struct StreamListener {
    rx: mpsc::Receiver<(TcpStream, SocketAddr)>,
    local_addr: SocketAddr,
}

impl StreamListener {
    /// Wraps `rx` so the worker's `axum::serve` loop reads accepted
    /// streams from it. `local_addr` is exposed verbatim to callers
    /// of [`axum::serve::Listener::local_addr`] - it is informational
    /// (typically the shared bind address) and not used for routing.
    #[must_use]
    pub(super) const fn new(
        rx: mpsc::Receiver<(TcpStream, SocketAddr)>,
        local_addr: SocketAddr,
    ) -> Self {
        Self { rx, local_addr }
    }
}

impl axum::serve::Listener for StreamListener {
    type Io = TcpStream;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            match self.rx.recv().await {
                Some(pair) => return pair,
                None => {
                    futures_pending().await;
                }
            }
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        Ok(self.local_addr)
    }
}

/// Awaits forever - used when the upstream sender side has dropped
/// (graceful shutdown). Avoids busy-spinning a closed channel.
async fn futures_pending() {
    std::future::pending::<()>().await;
}

#[cfg(test)]
mod tests {
    use super::StreamListener;
    use axum::serve::Listener;
    use std::net::SocketAddr;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn local_addr_is_passthrough() {
        let (_tx, rx) = mpsc::channel::<(tokio::net::TcpStream, SocketAddr)>(1);
        let bind: SocketAddr = "127.0.0.1:9000".parse().expect("addr");
        let listener = StreamListener::new(rx, bind);
        assert_eq!(listener.local_addr().expect("addr"), bind);
    }
}
