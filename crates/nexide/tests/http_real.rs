//! Integration tests for the synthetic `node:http` module: full
//! Node-shaped `createServer` / `IncomingMessage` / `ServerResponse`
//! round-trips plus `__nexide` handler-stack semantics (LIFO).
//!
//! Each test boots a real [`V8Engine`] with a CJS resolver pinned to
//! a temp directory, registers one or more handlers via either
//! `http.createServer(...).listen(0)` or `globalThis.__nexide.pushHandler`,
//! then drives [`RequestSlot`]s through `engine.enqueue` + `pump_once`
//! and asserts the assembled [`ResponsePayload`].

#![allow(
    clippy::future_not_send,
    clippy::significant_drop_tightening,
    clippy::doc_markdown
)]

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use nexide::engine::cjs::{FsResolver, ROOT_PARENT, default_registry};
use nexide::engine::{BootContext, V8Engine};
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

async fn dispatch(
    engine: &mut V8Engine,
    method: &str,
    uri: &str,
    body: &[u8],
) -> ResponsePayload {
    let meta = RequestMeta::try_new(method, uri).expect("meta");
    let slot = RequestSlot::new(
        meta,
        Vec::<HeaderPair>::new(),
        Bytes::copy_from_slice(body),
    );
    let mut rx = engine.enqueue(slot);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        engine.pump_once();
        match rx.try_recv() {
            Ok(result) => return result.expect("handler succeeded"),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                assert!(
                    std::time::Instant::now() <= deadline,
                    "handler did not complete within 5s",
                );
                tokio::time::sleep(Duration::from_millis(1)).await;
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

fn execute(engine: &mut V8Engine, source: &str) {
    engine.execute("[test:setup]", source).expect("execute");
    engine.pump_once();
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
async fn create_server_serves_request_via_handler_stack() {
    run_local(|| async {
        let mut booted = boot_with_entry(
            r"
            const http = require('node:http');
            const srv = http.createServer((req, res) => {
                res.writeHead(200, { 'content-type': 'text/plain' });
                res.end('hello via node:http (' + req.method + ' ' + req.url + ')');
            });
            srv.listen(0);
            globalThis.__primary = srv;
            ",
        )
        .await;

        let payload = dispatch(&mut booted.engine, "GET", "/probe", b"").await;
        assert_eq!(payload.head.status, 200);
        assert_eq!(header_value(&payload, "content-type"), Some("text/plain"));
        assert_eq!(&payload.body[..], b"hello via node:http (GET /probe)");
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn handler_stack_lifo_takeover_and_close_restores_previous() {
    run_local(|| async {
        let mut booted = boot_with_entry(
            r"
            const http = require('node:http');
            globalThis.__a = http.createServer((_req, res) => {
                res.writeHead(200, [['x-source','A']]);
                res.end('A');
            });
            globalThis.__a.listen(0);
            globalThis.__b = http.createServer((_req, res) => {
                res.writeHead(200, [['x-source','B']]);
                res.end('B');
            });
            globalThis.__b.listen(0);
            ",
        )
        .await;

        let p1 = dispatch(&mut booted.engine, "GET", "/", b"").await;
        assert_eq!(&p1.body[..], b"B");

        execute(&mut booted.engine, "globalThis.__b.close();");

        let p2 = dispatch(&mut booted.engine, "GET", "/", b"").await;
        assert_eq!(&p2.body[..], b"A");
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn server_response_headers_are_case_insensitive() {
    run_local(|| async {
        let mut booted = boot_with_entry(
            r"
            const http = require('node:http');
            http.createServer((_req, res) => {
                res.setHeader('X-Foo', 'one');
                const has = res.hasHeader('x-foo') ? '1' : '0';
                const got = res.getHeader('X-FOO') || '';
                res.removeHeader('X-FOO');
                const after = res.hasHeader('x-foo') ? '1' : '0';
                res.writeHead(200, [['content-type','text/plain']]);
                res.end(has + got + after);
            }).listen(0);
            ",
        )
        .await;

        let payload = dispatch(&mut booted.engine, "GET", "/", b"").await;
        assert_eq!(&payload.body[..], b"1one0");
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn write_head_after_send_throws_err_http_headers_sent() {
    run_local(|| async {
        let mut booted = boot_with_entry(
            r"
            const http = require('node:http');
            http.createServer((_req, res) => {
                res.writeHead(200, [['content-type','text/plain']]);
                res.write('chunk');
                try {
                    res.writeHead(500);
                    res.end('LEAK');
                } catch (err) {
                    res.end('|' + err.code);
                }
            }).listen(0);
            ",
        )
        .await;

        let payload = dispatch(&mut booted.engine, "GET", "/", b"").await;
        assert_eq!(&payload.body[..], b"chunk|ERR_HTTP_HEADERS_SENT");
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn incoming_message_is_async_iterable_readable() {
    run_local(|| async {
        let mut booted = boot_with_entry(
            r"
            const http = require('node:http');
            http.createServer(async (req, res) => {
                const chunks = [];
                for await (const c of req) chunks.push(c);
                const total = chunks.reduce((n, c) => n + c.byteLength, 0);
                const merged = new Uint8Array(total);
                let off = 0;
                for (const c of chunks) { merged.set(c, off); off += c.byteLength; }
                res.writeHead(200, [['content-type','application/octet-stream']]);
                res.end(Buffer.from(merged).toString('utf8'));
            }).listen(0);
            ",
        )
        .await;

        let payload = dispatch(&mut booted.engine, "POST", "/echo", b"hello-stream").await;
        assert_eq!(&payload.body[..], b"hello-stream");
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn http_request_rejects_missing_options() {
    run_local(|| async {
        let mut booted = boot_with_entry(
            r"
            const http = require('node:http');
            http.createServer((_req, res) => {
                let code = 'NOTHROW';
                try { http.request(); }
                catch (err) { code = err.code || err.name || 'NOCODE'; }
                res.writeHead(200, [['content-type','text/plain']]);
                res.end(code);
            }).listen(0);
            ",
        )
        .await;

        let payload = dispatch(&mut booted.engine, "GET", "/", b"").await;
        assert_ne!(&payload.body[..], b"NOTHROW");
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn net_module_is_a_safe_stub() {
    run_local(|| async {
        let mut booted = boot_with_entry(
            r"
            const net = require('node:net');
            const http = require('node:http');
            http.createServer((_req, res) => {
                const srv = net.createServer();
                srv.listen(0, '0.0.0.0', () => {
                    const out = [
                        net.isIPv4('127.0.0.1') ? '1' : '0',
                        net.isIPv6('::1') ? '1' : '0',
                        srv.address().address,
                    ].join('|');
                    res.writeHead(200, [['content-type','text/plain']]);
                    res.end(out);
                    srv.close();
                });
            }).listen(0);
            ",
        )
        .await;

        let payload = dispatch(&mut booted.engine, "GET", "/", b"").await;
        assert_eq!(&payload.body[..], b"1|1|0.0.0.0");
    })
    .await;
}
