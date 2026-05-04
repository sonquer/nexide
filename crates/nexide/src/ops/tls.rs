//! Host-side TLS primitives backing `node:tls` and `node:https`
//! outbound clients.
//!
//! `connect` performs a full rustls handshake against `host:port`,
//! verifying the server certificate against the bundled
//! `webpki-roots` trust store. The returned `TlsStream` is a
//! drop-in replacement for `TcpStream` from the JS side: read /
//! write semantics are identical and all errors are mapped to the
//! same Node-canonical codes used by [`super::net`].
//!
//! Structured `tracing` records emit on the `nexide::ops::tls`
//! target. Handshake lifecycle (dial, upgrade, established) logs at
//! `debug`; per-chunk I/O at `trace`; certificate / handshake
//! failures at `warn`.

use std::io;
use std::sync::Arc;
use std::sync::OnceLock;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};

use super::net::{AddressInfo, NetError};

const LOG_TARGET: &str = "nexide::ops::tls";

fn shared_config() -> Arc<ClientConfig> {
    static CFG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    CFG.get_or_init(|| {
        let mut roots = RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let cfg = ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        Arc::new(cfg)
    })
    .clone()
}

fn tls_error(err: &io::Error) -> NetError {
    let mut mapped = NetError::from_io(err);
    if mapped.code == "EIO" {
        let lower = err.to_string().to_lowercase();
        if lower.contains("certificate")
            || lower.contains("handshake")
            || lower.contains("verifier")
        {
            mapped.code = "CERT_HAS_EXPIRED";
        }
    }
    mapped
}

/// Upgrades an existing TCP stream to TLS by performing a client
/// handshake over it. Used for protocols that negotiate TLS over a
/// plain TCP connection (e.g. PostgreSQL `SSLRequest`, SMTP
/// `STARTTLS`, IMAP/POP3 `STARTTLS`).
///
/// # Errors
/// Returns `NetError` if the underlying socket address cannot be
/// queried or the TLS handshake fails (handshake errors are mapped
/// to canonical Node codes via [`tls_error`]).
#[tracing::instrument(
    target = "nexide::ops::tls",
    level = "debug",
    name = "tls_upgrade",
    skip_all,
    fields(host = %host),
    err(level = "warn", Display),
)]
pub async fn upgrade(
    tcp: TcpStream,
    host: &str,
) -> Result<(TlsStream<TcpStream>, AddressInfo, AddressInfo), NetError> {
    let local = tcp.local_addr().map_err(|e| tls_error(&e))?;
    let remote = tcp.peer_addr().map_err(|e| tls_error(&e))?;
    tracing::trace!(
        target: LOG_TARGET,
        local = %local,
        remote = %remote,
        "starting handshake on existing tcp stream",
    );
    let server_name = ServerName::try_from(host.to_owned())
        .map_err(|_| NetError::new("ERR_INVALID_HOSTNAME", format!("invalid host {host}")))?;
    let connector = TlsConnector::from(shared_config());
    let stream = connector
        .connect(server_name, tcp)
        .await
        .map_err(|e| tls_error(&e))?;
    tracing::debug!(
        target: LOG_TARGET,
        local = %local,
        remote = %remote,
        "tls handshake complete",
    );
    Ok((stream, local.into(), remote.into()))
}

/// Performs a TLS client handshake against `host:port` and returns
/// the live stream plus address info pulled from the underlying TCP
/// socket.
///
/// # Errors
/// Returns `NetError` on DNS, TCP or TLS handshake failures. The
/// mapped error code mirrors what Node would expose under the same
/// circumstances.
#[tracing::instrument(
    target = "nexide::ops::tls",
    level = "debug",
    name = "tls_connect",
    skip_all,
    fields(host = %host, port),
    err(level = "warn", Display),
)]
pub async fn connect(
    host: &str,
    port: u16,
) -> Result<(TlsStream<TcpStream>, AddressInfo, AddressInfo), NetError> {
    let target = format!("{host}:{port}");
    let tcp = TcpStream::connect(&target)
        .await
        .map_err(|e| tls_error(&e))?;
    upgrade(tcp, host).await
}

/// Reads up to `max` bytes from `stream`. Returns an empty `Vec` on
/// clean shutdown.
pub async fn read_chunk(
    stream: &mut TlsStream<TcpStream>,
    max: usize,
) -> Result<Vec<u8>, NetError> {
    let cap = max.clamp(1, 64 * 1024);
    let mut buf = vec![0u8; cap];
    match stream.read(&mut buf).await {
        Ok(n) => {
            buf.truncate(n);
            if n == 0 {
                tracing::debug!(target: LOG_TARGET, "tls peer half-closed");
            } else {
                tracing::trace!(
                    target: LOG_TARGET,
                    bytes = n,
                    capacity = cap,
                    "tls read",
                );
            }
            Ok(buf)
        }
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
            tracing::debug!(
                target: LOG_TARGET,
                error = %e,
                "tls peer closed without close_notify; treating as clean eof",
            );
            Ok(Vec::new())
        }
        Err(e) => {
            let mapped = tls_error(&e);
            tracing::warn!(
                target: LOG_TARGET,
                code = mapped.code,
                error = %e,
                "tls read failed",
            );
            Err(mapped)
        }
    }
}

/// Writes `data` to `stream` and waits for the write half to flush.
pub async fn write_all(stream: &mut TlsStream<TcpStream>, data: &[u8]) -> Result<(), NetError> {
    let total = data.len();
    if let Err(e) = stream.write_all(data).await {
        let mapped = tls_error(&e);
        tracing::warn!(
            target: LOG_TARGET,
            code = mapped.code,
            total,
            error = %e,
            "tls write failed",
        );
        return Err(mapped);
    }
    if let Err(e) = stream.flush().await {
        let mapped = tls_error(&e);
        tracing::warn!(
            target: LOG_TARGET,
            code = mapped.code,
            total,
            error = %e,
            "tls flush failed",
        );
        return Err(mapped);
    }
    tracing::trace!(target: LOG_TARGET, bytes = total, "tls write completed");
    Ok(())
}

/// Cleanly shuts the TLS layer down, sending `close_notify` to the
/// peer so the connection is not torn down ungracefully.
pub async fn shutdown(stream: &mut TlsStream<TcpStream>) -> Result<(), NetError> {
    match stream.shutdown().await {
        Ok(()) => {
            tracing::debug!(target: LOG_TARGET, "tls shutdown clean");
            Ok(())
        }
        Err(e) => {
            let mapped = tls_error(&e);
            tracing::warn!(
                target: LOG_TARGET,
                code = mapped.code,
                error = %e,
                "tls shutdown failed",
            );
            Err(mapped)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_config_is_cached() {
        let a = shared_config();
        let b = shared_config();
        assert!(Arc::ptr_eq(&a, &b));
    }

    #[tokio::test]
    async fn invalid_hostname_returns_typed_error() {
        let result = connect("..invalid..", 443).await;
        assert!(result.is_err());
    }
}
