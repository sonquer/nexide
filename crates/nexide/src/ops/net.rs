//! Host-side TCP primitives backing `node:net`.
//!
//! Each function is a thin façade over `tokio::net` so the JS-facing
//! ops in [`super::super::engine::v8_engine::ops_bridge`] can stay
//! free of I/O details. Errors are mapped to Node-canonical codes
//! (`ECONNREFUSED`, `ETIMEDOUT`, …) so JavaScript can pattern-match
//! on `err.code`.
//!
//! All public entry points emit structured `tracing` records on the
//! `nexide::ops::net` target. Lifecycle events (connect, listen,
//! accept, close) are logged at `debug`; per-chunk I/O at `trace`;
//! recoverable failures at `warn`; transport corruption at `error`.

use std::fmt;
use std::io;
use std::net::SocketAddr;

use tokio::net::{TcpListener, TcpStream};

const LOG_TARGET: &str = "nexide::ops::net";

/// Node-shaped error: a string code (`ECONNREFUSED`, `ENOTFOUND`,
/// …) plus a human-readable message.
#[derive(Debug, Clone)]
pub struct NetError {
    /// Node-canonical error code (`ECONNREFUSED`, `EADDRINUSE`, …).
    pub code: &'static str,
    /// Human-readable description, suitable for `Error.message`.
    pub message: String,
}

impl NetError {
    /// Builds an error from a code + message pair.
    #[must_use]
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// Maps a `std::io::Error` onto a Node-canonical error code.
    #[must_use]
    pub fn from_io(err: &io::Error) -> Self {
        let code = match err.kind() {
            io::ErrorKind::ConnectionRefused => "ECONNREFUSED",
            io::ErrorKind::ConnectionReset => "ECONNRESET",
            io::ErrorKind::ConnectionAborted => "ECONNABORTED",
            io::ErrorKind::NotConnected => "ENOTCONN",
            io::ErrorKind::AddrInUse => "EADDRINUSE",
            io::ErrorKind::AddrNotAvailable => "EADDRNOTAVAIL",
            io::ErrorKind::BrokenPipe => "EPIPE",
            io::ErrorKind::AlreadyExists => "EEXIST",
            io::ErrorKind::WouldBlock => "EAGAIN",
            io::ErrorKind::TimedOut => "ETIMEDOUT",
            io::ErrorKind::Interrupted => "EINTR",
            io::ErrorKind::PermissionDenied => "EACCES",
            io::ErrorKind::UnexpectedEof => "ECONNRESET",
            _ => "EIO",
        };
        Self {
            code,
            message: err.to_string(),
        }
    }
}

impl From<io::Error> for NetError {
    fn from(value: io::Error) -> Self {
        Self::from_io(&value)
    }
}

impl fmt::Display for NetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for NetError {}

/// Address summary (`host`, `port`, `family`) returned by the
/// `address()` and connection-establishment ops.
#[derive(Debug, Clone)]
pub struct AddressInfo {
    /// Numeric IP address as a string (`"127.0.0.1"`, `"::1"`).
    pub address: String,
    /// TCP port number.
    pub port: u16,
    /// `4` for IPv4, `6` for IPv6.
    pub family: u8,
}

impl From<SocketAddr> for AddressInfo {
    fn from(addr: SocketAddr) -> Self {
        Self {
            address: addr.ip().to_string(),
            port: addr.port(),
            family: if addr.is_ipv4() { 4 } else { 6 },
        }
    }
}

impl fmt::Display for AddressInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.family == 6 {
            write!(f, "[{}]:{}", self.address, self.port)
        } else {
            write!(f, "{}:{}", self.address, self.port)
        }
    }
}

/// Opens an outbound TCP connection.
///
/// `host` is resolved through the OS resolver; the first address
/// reachable is used. Returns `(stream, local, remote)` so the caller
/// can populate the JS-facing socket properties immediately.
#[tracing::instrument(
    target = "nexide::ops::net",
    level = "debug",
    name = "tcp_connect",
    skip_all,
    fields(host = %host, port),
    err(level = "warn", Display),
)]
pub async fn connect(
    host: &str,
    port: u16,
) -> Result<(TcpStream, AddressInfo, AddressInfo), NetError> {
    let target = format!("{host}:{port}");
    tracing::trace!(target: LOG_TARGET, target_addr = %target, "dialing");
    let stream = TcpStream::connect(&target).await?;
    let local = stream.local_addr()?;
    let remote = stream.peer_addr()?;
    tracing::debug!(
        target: LOG_TARGET,
        local = %local,
        remote = %remote,
        "tcp connection established",
    );
    Ok((stream, local.into(), remote.into()))
}

/// Binds a TCP listener on `host:port`. Use `host = "0.0.0.0"` for
/// dual-stack semantics that mirror Node's defaults.
#[tracing::instrument(
    target = "nexide::ops::net",
    level = "debug",
    name = "tcp_listen",
    skip_all,
    fields(host = %host, port),
    err(level = "warn", Display),
)]
pub async fn listen(host: &str, port: u16) -> Result<(TcpListener, AddressInfo), NetError> {
    let target = format!("{host}:{port}");
    let listener = TcpListener::bind(&target).await?;
    let local = listener.local_addr()?;
    tracing::debug!(target: LOG_TARGET, local = %local, "tcp listener bound");
    Ok((listener, local.into()))
}

/// Awaits the next inbound connection on `listener`.
#[tracing::instrument(
    target = "nexide::ops::net",
    level = "trace",
    name = "tcp_accept",
    skip_all,
    err(level = "warn", Display)
)]
pub async fn accept(
    listener: &TcpListener,
) -> Result<(TcpStream, AddressInfo, AddressInfo), NetError> {
    let (stream, peer) = listener.accept().await?;
    let local = stream.local_addr()?;
    tracing::debug!(
        target: LOG_TARGET,
        local = %local,
        remote = %peer,
        "tcp connection accepted",
    );
    Ok((stream, local.into(), peer.into()))
}

/// Reads up to `max` bytes from `stream`. Returns an empty `Vec` on
/// EOF - JavaScript can detect the half-close by checking `len === 0`.
///
/// Uses `readable().await` + `try_read` so reading does not require
/// exclusive ownership of the stream and concurrent writes can make
/// progress on the same FD (Node's net.Socket semantics).
pub async fn read_chunk(stream: &TcpStream, max: usize) -> Result<Vec<u8>, NetError> {
    let cap = max.clamp(1, 64 * 1024);
    let mut buf = vec![0u8; cap];
    let trace_enabled = tracing::enabled!(target: LOG_TARGET, tracing::Level::TRACE);
    let mut spurious_wakeups: u32 = 0;
    loop {
        stream.readable().await?;
        match stream.try_read(&mut buf) {
            Ok(n) => {
                buf.truncate(n);
                if n == 0 {
                    tracing::debug!(target: LOG_TARGET, "tcp peer half-closed");
                } else if trace_enabled {
                    tracing::trace!(
                        target: LOG_TARGET,
                        bytes = n,
                        capacity = cap,
                        spurious_wakeups,
                        "tcp read",
                    );
                }
                return Ok(buf);
            }
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                if trace_enabled {
                    spurious_wakeups = spurious_wakeups.saturating_add(1);
                }
                continue;
            }
            Err(e) => {
                let mapped = NetError::from_io(&e);
                tracing::warn!(
                    target: LOG_TARGET,
                    code = mapped.code,
                    error = %e,
                    "tcp read failed",
                );
                return Err(mapped);
            }
        }
    }
}

/// Writes `data` to `stream`, flushing the kernel send buffer. Node
/// callers only ever observe a successful write or a definitive
/// failure; partial writes are masked by the loop.
///
/// Uses `writable().await` + `try_write` so writing does not require
/// exclusive ownership of the stream and concurrent reads can make
/// progress on the same FD.
pub async fn write_all(stream: &TcpStream, data: &[u8]) -> Result<(), NetError> {
    let total = data.len();
    let mut written = 0usize;
    let trace_enabled = tracing::enabled!(target: LOG_TARGET, tracing::Level::TRACE);
    let mut iterations: u32 = 0;
    while written < total {
        stream.writable().await?;
        match stream.try_write(&data[written..]) {
            Ok(0) => {
                tracing::warn!(
                    target: LOG_TARGET,
                    written,
                    total,
                    "tcp write returned 0 bytes; treating as EPIPE",
                );
                return Err(NetError::new("EPIPE", "tcp write returned 0 bytes"));
            }
            Ok(n) => written += n,
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                if trace_enabled {
                    iterations = iterations.saturating_add(1);
                }
                continue;
            }
            Err(e) => {
                let mapped = NetError::from_io(&e);
                tracing::warn!(
                    target: LOG_TARGET,
                    code = mapped.code,
                    written,
                    total,
                    error = %e,
                    "tcp write failed",
                );
                return Err(mapped);
            }
        }
    }
    if trace_enabled {
        tracing::trace!(
            target: LOG_TARGET,
            bytes = total,
            iterations,
            "tcp write completed",
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn listen_and_connect_round_trip() {
        let (listener, addr) = listen("127.0.0.1", 0).await.expect("listen");
        let port = addr.port;
        let server = tokio::spawn(async move {
            let (s, _, _) = accept(&listener).await.expect("accept");
            write_all(&s, b"hello").await.expect("write");
        });
        let (client, _, _) = connect("127.0.0.1", port).await.expect("connect");
        let chunk = read_chunk(&client, 64).await.expect("read");
        assert_eq!(chunk, b"hello");
        server.await.expect("join");
    }

    #[test]
    fn from_io_maps_refused() {
        let err = io::Error::from(io::ErrorKind::ConnectionRefused);
        assert_eq!(NetError::from_io(&err).code, "ECONNREFUSED");
    }
}
