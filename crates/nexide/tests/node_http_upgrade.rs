//! Integration tests for the HTTP/1.1 `Upgrade` raw-socket bridge.
//!
//! These tests boot a real V8 engine, register a `node:http` Server
//! with an `'upgrade'` listener, and verify two things end-to-end:
//!
//! 1. When a request carrying the synthetic
//!    `x-nexide-upgrade-socket-id` header arrives, the JS adapter
//!    routes it to `'upgrade'` listeners (not `'request'` listeners)
//!    and exposes a Duplex socket bound to the registry.
//! 2. When the listener writes an HTTP/1.1 101 response head onto the
//!    socket (the `ws` library's pattern), the bytes are parsed and
//!    converted into a real `synthRes.writeHead(101, …)` +
//!    `synthRes.end()` so the Rust shield emits the 101 on the wire.
//!
//! Post-handshake byte plumbing is exercised by the Rust-side unit
//! tests in `ops::upgrade_socket::tests`; here we focus on the
//! JS-facing handshake commit.

#![allow(clippy::future_not_send, clippy::doc_markdown)]

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use nexide::engine::cjs::{FsResolver, ROOT_PARENT, default_registry};
use nexide::engine::{BootContext, V8Engine};
use nexide::ops::upgrade_socket;
use nexide::ops::{HeaderPair, RequestMeta, RequestSlot, ResponsePayload};
use tempfile::TempDir;

struct Booted {
    engine: V8Engine,
    _sandbox: TempDir,
}

async fn boot_with_entry(body: &str) -> Booted {
    let sandbox = tempfile::tempdir().expect("tempdir");
    let entry = sandbox.path().join("entry.cjs");
    std::fs::write(&entry, body).expect("write entry");
    let registry = Arc::new(default_registry().expect("registry"));
    let resolver = Arc::new(FsResolver::new(
        vec![sandbox.path().to_path_buf()],
        registry,
    ));
    let ctx = BootContext::new()
        .with_cjs(resolver)
        .with_cjs_root(ROOT_PARENT);
    let mut engine = V8Engine::boot_with(&entry, ctx).await.expect("boot");
    engine.start_pump(0).expect("start pump");
    Booted {
        engine,
        _sandbox: sandbox,
    }
}

async fn dispatch_with_headers(
    engine: &mut V8Engine,
    method: &str,
    uri: &str,
    headers: Vec<HeaderPair>,
) -> ResponsePayload {
    let meta = RequestMeta::try_new(method, uri).expect("meta");
    let slot = RequestSlot::new(meta, headers, Bytes::new());
    let mut rx = engine.enqueue(slot);
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        engine.pump_once();
        match rx.try_recv() {
            Ok(result) => return result.expect("handler succeeded"),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                assert!(
                    std::time::Instant::now() <= deadline,
                    "handler did not complete within 10s",
                );
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
            Err(other) => panic!("oneshot closed: {other:?}"),
        }
    }
}

fn header_value<'a>(payload: &'a ResponsePayload, name: &str) -> Option<&'a str> {
    payload
        .head
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

async fn run_local<F, Fut, T>(f: F) -> T
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    let local = tokio::task::LocalSet::new();
    local.run_until(f()).await
}

#[tokio::test(flavor = "current_thread")]
async fn upgrade_listener_handshake_commits_101_response() {
    run_local(|| async {
        // Pre-allocate a socket id on the Rust side so the JS adapter
        // sees a valid registry slot. In production this is done by
        // `next_bridge::build_proto_request`.
        let socket = upgrade_socket::allocate();
        let socket_id = socket.id();
        // Drop the handle: we don't drive I/O from the test harness.
        // The slot stays alive in the registry until JS closes it or
        // `abort` is called.
        drop(socket);

        let mut booted = boot_with_entry(
            r"
            const http = require('node:http');
            const srv = http.createServer();
            srv.on('upgrade', (req, socket, head) => {
                // Mirror the `ws` library's pattern: write the 101
                // status + handshake headers as one CRLF-terminated
                // buffer onto the raw socket.
                const accept = 'computed-accept-key';
                socket.write(
                    'HTTP/1.1 101 Switching Protocols\r\n' +
                    'Upgrade: websocket\r\n' +
                    'Connection: Upgrade\r\n' +
                    'Sec-WebSocket-Accept: ' + accept + '\r\n' +
                    '\r\n'
                );
            });
            srv.listen(0);
            globalThis.__srv = srv;
            ",
        )
        .await;

        let headers = vec![
            HeaderPair {
                name: "upgrade".to_owned(),
                value: "websocket".to_owned(),
            },
            HeaderPair {
                name: "connection".to_owned(),
                value: "Upgrade".to_owned(),
            },
            HeaderPair {
                name: "sec-websocket-key".to_owned(),
                value: "dGhlIHNhbXBsZSBub25jZQ==".to_owned(),
            },
            HeaderPair {
                name: "sec-websocket-version".to_owned(),
                value: "13".to_owned(),
            },
            HeaderPair {
                name: upgrade_socket::UPGRADE_SOCKET_ID_HEADER.to_owned(),
                value: socket_id.to_string(),
            },
        ];

        let payload = dispatch_with_headers(&mut booted.engine, "GET", "/ws", headers).await;
        assert_eq!(payload.head.status, 101);
        assert_eq!(header_value(&payload, "upgrade"), Some("websocket"));
        assert_eq!(header_value(&payload, "connection"), Some("Upgrade"));
        assert_eq!(
            header_value(&payload, "sec-websocket-accept"),
            Some("computed-accept-key"),
        );
        assert!(payload.body.is_empty(), "101 must have empty body");

        // Cleanup: close the slot so we don't leak it across tests.
        if let Some(handle) = upgrade_socket::handle(socket_id) {
            handle.close();
        }
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn upgrade_without_socket_id_falls_back_to_501() {
    run_local(|| async {
        let mut booted = boot_with_entry(
            r"
            const http = require('node:http');
            const srv = http.createServer();
            srv.on('upgrade', (req, socket, head) => {
                // Listener present but socket is null — the adapter
                // must close out the response cleanly.
            });
            srv.listen(0);
            globalThis.__srv = srv;
            ",
        )
        .await;

        let headers = vec![
            HeaderPair {
                name: "upgrade".to_owned(),
                value: "websocket".to_owned(),
            },
            HeaderPair {
                name: "connection".to_owned(),
                value: "Upgrade".to_owned(),
            },
        ];

        let payload = dispatch_with_headers(&mut booted.engine, "GET", "/ws", headers).await;
        assert_eq!(payload.head.status, 501);
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn non_upgrade_request_with_synthetic_header_is_ignored() {
    // Sanity check: the synthetic header alone (without `Upgrade`)
    // must not be enough to trigger the upgrade path. In production
    // `next_bridge` only injects the header when both `Upgrade` and
    // `Connection: upgrade` are present, but we verify the JS adapter
    // is defensively gated as well.
    run_local(|| async {
        let socket = upgrade_socket::allocate();
        let socket_id = socket.id();
        drop(socket);

        let mut booted = boot_with_entry(
            r"
            const http = require('node:http');
            const srv = http.createServer((req, res) => {
                res.writeHead(200, { 'content-type': 'text/plain' });
                res.end('plain');
            });
            srv.on('upgrade', () => { throw new Error('upgrade should not fire'); });
            srv.listen(0);
            globalThis.__srv = srv;
            ",
        )
        .await;

        let headers = vec![HeaderPair {
            name: upgrade_socket::UPGRADE_SOCKET_ID_HEADER.to_owned(),
            value: socket_id.to_string(),
        }];

        let payload = dispatch_with_headers(&mut booted.engine, "GET", "/x", headers).await;
        assert_eq!(payload.head.status, 200);
        assert_eq!(&payload.body[..], b"plain");

        if let Some(handle) = upgrade_socket::handle(socket_id) {
            handle.close();
        }
    })
    .await;
}
