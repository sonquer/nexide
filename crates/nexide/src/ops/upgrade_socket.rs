//! Raw post-handshake socket bridge for HTTP `Upgrade` requests
//! (WebSockets, HTTP/2 cleartext upgrade, custom protocols).
//!
//! ## Why this exists
//!
//! Node's `node:http` Server emits an `'upgrade'` event with a *raw*
//! `net.Socket` so libraries like `ws` and `socket.io` can drive
//! their own framing on top of the post-101 byte stream. nexide
//! terminates HTTP/1.1 at hyper, so by the time JS sees a request
//! the connection is already inside an HTTP framer; the `Upgraded`
//! handle is only obtainable *after* hyper writes a 101 response.
//!
//! This module bridges the gap. Each upgrade request is registered
//! with a fresh socket id; JS interacts with the registry via three
//! ops:
//!
//!   - [`op_upgrade_socket_write_async`](super::super::engine::v8_engine::ops_bridge)
//!     buffers bytes in a shared queue. Until the upgrade completes
//!     (the server has flushed 101 to the wire and `OnUpgrade` has
//!     resolved) writes accumulate in `pending_writes`; once
//!     [`attach_upgraded`] is called the buffer is drained onto the
//!     real upgraded stream and subsequent writes are forwarded
//!     directly.
//!
//!   - `op_upgrade_socket_read_async` pulls one chunk from the
//!     inbound channel; before the upgrade resolves it parks until
//!     [`attach_upgraded`] runs; afterwards it returns frame-agnostic
//!     bytes from the upgraded stream.
//!
//!   - `op_upgrade_socket_close` removes the slot, dropping driver
//!     tasks which closes both halves of the socket cleanly.
//!
//! ## Lifecycle
//!
//! 1. The HTTP shield ([`crate::server::next_bridge`]) detects an
//!    upgrade request, calls [`allocate`] to reserve an id, and
//!    injects `x-nexide-upgrade-socket-id: <id>` into the
//!    [`crate::dispatch::ProtoRequest`] passed to JS.
//! 2. The shield removes [`hyper::upgrade::OnUpgrade`] from the
//!    request extensions and spawns a task that awaits it. On
//!    success it calls [`attach_upgraded`]; on failure it calls
//!    [`abort`] so JS-side reads / writes resolve with EPIPE.
//! 3. The JS handler emits `'upgrade'` and synthesises a 101
//!    response using `res.writeHead(101, …)` so hyper sends the
//!    101 on the wire. That flush is what causes `OnUpgrade` to
//!    resolve.
//! 4. JS runs its protocol (ws-frames, etc.) entirely on top of the
//!    socket-id read/write ops.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use hyper::upgrade::Upgraded;
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Notify, mpsc};

const LOG_TARGET: &str = "nexide::ops::upgrade_socket";
const READ_CHUNK_BYTES: usize = 16 * 1024;

/// Outcome of a JS-side read/write op when the slot is unknown.
#[derive(Debug, thiserror::Error)]
pub enum UpgradeSocketError {
    /// Socket id was never allocated, or has been closed.
    #[error("upgrade socket {0} is closed")]
    Closed(u64),
    /// The OnUpgrade future failed before the socket was attached.
    #[error("upgrade socket {0} aborted: {1}")]
    Aborted(u64, String),
}

/// JS-facing handle to one upgrade socket.
pub struct UpgradeSocketHandle {
    id: u64,
    inner: Arc<UpgradeSocketInner>,
}

impl UpgradeSocketHandle {
    /// Returns the numeric socket id (used as the JS-facing key).
    #[must_use]
    pub const fn id(&self) -> u64 {
        self.id
    }

    /// Schedules `bytes` for delivery on the socket.
    ///
    /// Before the upgrade has resolved the bytes are queued in the
    /// pre-handshake buffer; after [`attach_upgraded`] runs they go
    /// directly to the upgraded stream.
    pub async fn write(&self, bytes: Vec<u8>) -> Result<(), UpgradeSocketError> {
        if self.inner.closed.load(Ordering::Acquire) {
            return Err(UpgradeSocketError::Closed(self.id));
        }
        if let Some(err) = self
            .inner
            .error
            .lock()
            .expect("upgrade-socket lock")
            .clone()
        {
            return Err(UpgradeSocketError::Aborted(self.id, err));
        }
        if self.inner.attached.load(Ordering::Acquire) {
            self.inner
                .out_tx
                .lock()
                .expect("upgrade-socket lock")
                .as_ref()
                .ok_or(UpgradeSocketError::Closed(self.id))?
                .send(bytes)
                .map_err(|_| UpgradeSocketError::Closed(self.id))?;
            return Ok(());
        }
        self.inner
            .pending_writes
            .lock()
            .expect("upgrade-socket lock")
            .push_back(bytes);
        Ok(())
    }

    /// Reads up to one chunk from the inbound stream. `Ok(None)`
    /// signals graceful EOF.
    pub async fn read(&self) -> Result<Option<Vec<u8>>, UpgradeSocketError> {
        if self.inner.closed.load(Ordering::Acquire) {
            return Err(UpgradeSocketError::Closed(self.id));
        }
        if !self.inner.attached.load(Ordering::Acquire) {
            self.inner.attached_notify.notified().await;
            if let Some(err) = self
                .inner
                .error
                .lock()
                .expect("upgrade-socket lock")
                .clone()
            {
                return Err(UpgradeSocketError::Aborted(self.id, err));
            }
            if self.inner.closed.load(Ordering::Acquire) {
                return Err(UpgradeSocketError::Closed(self.id));
            }
        }
        let mut guard = self.inner.in_rx.lock().await;
        let rx = guard.as_mut().ok_or(UpgradeSocketError::Closed(self.id))?;
        Ok(rx.recv().await)
    }

    /// Marks the socket as closed and drops its driver tasks.
    pub fn close(&self) {
        self.inner.closed.store(true, Ordering::Release);

        self.inner
            .out_tx
            .lock()
            .expect("upgrade-socket lock")
            .take();

        self.inner.attached_notify.notify_waiters();
        unregister(self.id);
    }
}

struct UpgradeSocketInner {
    attached: AtomicBool,
    closed: AtomicBool,
    attached_notify: Notify,
    /// Pre-handshake outbound buffer. Drained onto the upgraded
    /// stream by `attach_upgraded` once the 101 has been flushed.
    pending_writes: Mutex<VecDeque<Vec<u8>>>,
    /// Post-handshake outbound channel; producer side. Replaced by
    /// `attach_upgraded`. `None` once closed.
    out_tx: Mutex<Option<mpsc::UnboundedSender<Vec<u8>>>>,
    /// Inbound chunks. Populated by the driver task spawned in
    /// `attach_upgraded`.
    in_rx: tokio::sync::Mutex<Option<mpsc::UnboundedReceiver<Vec<u8>>>>,
    in_tx: Mutex<Option<mpsc::UnboundedSender<Vec<u8>>>>,
    /// Captured driver error (e.g. OnUpgrade failure or transport
    /// error). Surfaced to JS through the next read/write op.
    error: Mutex<Option<String>>,
}

static REGISTRY: LazyLock<Mutex<HashMap<u64, Arc<UpgradeSocketInner>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn register(inner: Arc<UpgradeSocketInner>) -> u64 {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    REGISTRY
        .lock()
        .expect("upgrade-socket registry")
        .insert(id, inner);
    id
}

fn unregister(id: u64) {
    REGISTRY
        .lock()
        .expect("upgrade-socket registry")
        .remove(&id);
}

fn lookup(id: u64) -> Option<Arc<UpgradeSocketInner>> {
    REGISTRY
        .lock()
        .expect("upgrade-socket registry")
        .get(&id)
        .cloned()
}

/// Reserves a fresh socket id and returns the JS-facing handle.
/// The id is the integer the shield injects into request headers.
#[must_use]
pub fn allocate() -> UpgradeSocketHandle {
    let inner = Arc::new(UpgradeSocketInner {
        attached: AtomicBool::new(false),
        closed: AtomicBool::new(false),
        attached_notify: Notify::new(),
        pending_writes: Mutex::new(VecDeque::new()),
        out_tx: Mutex::new(None),
        in_rx: tokio::sync::Mutex::new(None),
        in_tx: Mutex::new(None),
        error: Mutex::new(None),
    });
    let id = register(Arc::clone(&inner));
    tracing::debug!(target: LOG_TARGET, socket_id = id, "upgrade socket allocated");
    UpgradeSocketHandle { id, inner }
}

/// Returns a handle for an existing socket id. Returns `None` if
/// the slot has been closed or never allocated.
#[must_use]
pub fn handle(id: u64) -> Option<UpgradeSocketHandle> {
    lookup(id).map(|inner| UpgradeSocketHandle { id, inner })
}

/// Drives the post-handshake socket: spawns one task that pumps
/// inbound bytes from the upgraded stream into the JS-side channel
/// and another that pumps JS-side writes back onto the wire. Drains
/// any pre-handshake buffered writes onto the wire first so the
/// `ws` library's "write 101 + immediately send first frame" idiom
/// works.
///
/// # Panics
///
/// Panics if `id` does not correspond to a previously allocated
/// socket.
pub fn attach_upgraded(id: u64, upgraded: Upgraded) {
    let Some(inner) = lookup(id) else {
        tracing::warn!(target: LOG_TARGET, socket_id = id, "attach on unknown socket");
        return;
    };
    if inner.closed.load(Ordering::Acquire) {
        return;
    }
    let pending: VecDeque<Vec<u8>> =
        std::mem::take(&mut *inner.pending_writes.lock().expect("upgrade-socket lock"));

    let (out_tx, out_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let (in_tx, in_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    *inner.out_tx.lock().expect("upgrade-socket lock") = Some(out_tx);
    *inner.in_tx.lock().expect("upgrade-socket lock") = Some(in_tx.clone());
    {
        let mut guard = inner
            .in_rx
            .try_lock()
            .expect("upgrade-socket in_rx must be untouched until attached_notify fires");
        *guard = Some(in_rx);
    }

    let io = TokioIo::new(upgraded);
    let (read_half, write_half) = tokio::io::split(io);
    spawn_reader(id, Arc::clone(&inner), read_half);
    spawn_writer(id, Arc::clone(&inner), write_half, pending, out_rx);

    inner.attached.store(true, Ordering::Release);
    inner.attached_notify.notify_waiters();
    tracing::debug!(target: LOG_TARGET, socket_id = id, "upgrade socket attached");
}

/// Marks the socket as failed (`OnUpgrade` resolved with an error).
/// Pending JS reads/writes complete with [`UpgradeSocketError::Aborted`].
pub fn abort(id: u64, reason: String) {
    if let Some(inner) = lookup(id) {
        *inner.error.lock().expect("upgrade-socket lock") = Some(reason);
        inner.closed.store(true, Ordering::Release);
        inner.attached_notify.notify_waiters();
    }
    unregister(id);
}

fn spawn_reader<R>(id: u64, inner: Arc<UpgradeSocketInner>, mut read_half: R)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let in_tx = match inner.in_tx.lock().expect("upgrade-socket lock").clone() {
            Some(tx) => tx,
            None => return,
        };
        let mut buf = vec![0u8; READ_CHUNK_BYTES];
        loop {
            if inner.closed.load(Ordering::Acquire) {
                break;
            }
            match read_half.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    if in_tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(err) => {
                    *inner.error.lock().expect("upgrade-socket lock") = Some(err.to_string());
                    break;
                }
            }
        }
        // Closing in_tx by dropping it signals EOF to the JS reader.
        drop(in_tx);
        inner.in_tx.lock().expect("upgrade-socket lock").take();
        tracing::debug!(target: LOG_TARGET, socket_id = id, "upgrade socket reader done");
    });
}

fn spawn_writer<W>(
    id: u64,
    inner: Arc<UpgradeSocketInner>,
    mut write_half: W,
    pending: VecDeque<Vec<u8>>,
    mut out_rx: mpsc::UnboundedReceiver<Vec<u8>>,
) where
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        for chunk in pending {
            if let Err(err) = write_half.write_all(&chunk).await {
                *inner.error.lock().expect("upgrade-socket lock") = Some(err.to_string());
                return;
            }
        }
        while let Some(chunk) = out_rx.recv().await {
            if let Err(err) = write_half.write_all(&chunk).await {
                *inner.error.lock().expect("upgrade-socket lock") = Some(err.to_string());
                break;
            }
        }
        let _ = write_half.shutdown().await;
        tracing::debug!(target: LOG_TARGET, socket_id = id, "upgrade socket writer done");
    });
}

/// Synthetic header injected into the JS-facing request so the
/// `node:http` adapter can route the request to the `'upgrade'`
/// listener instead of the regular request handler.
pub const UPGRADE_SOCKET_ID_HEADER: &str = "x-nexide-upgrade-socket-id";

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn pre_handshake_writes_buffer_until_attach() {
        let h = allocate();
        h.write(b"first".to_vec()).await.unwrap();
        h.write(b"second".to_vec()).await.unwrap();
        assert!(!h.inner.attached.load(Ordering::Acquire));
        let pending = h.inner.pending_writes.lock().unwrap();
        assert_eq!(pending.len(), 2);
    }

    #[tokio::test]
    async fn close_marks_slot_unreachable() {
        let h = allocate();
        let id = h.id();
        h.close();
        assert!(handle(id).is_none());
    }

    #[tokio::test]
    async fn abort_surfaces_error_to_reader() {
        let h = allocate();
        let id = h.id();
        let reader = tokio::spawn(async move { h.read().await });
        // Give the reader a moment to park.
        tokio::task::yield_now().await;
        abort(id, "onupgrade failed".to_owned());
        let res = reader.await.unwrap();
        match res {
            Err(UpgradeSocketError::Aborted(got_id, msg)) => {
                assert_eq!(got_id, id);
                assert_eq!(msg, "onupgrade failed");
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }
}
