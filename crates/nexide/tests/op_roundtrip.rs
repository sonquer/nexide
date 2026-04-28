//! End-to-end op-bridge round-trip: plant a [`RequestSlot`] in a
//! freshly-booted [`V8Engine`], have the JS pump dispatch it through
//! a synthetic handler installed via `globalThis.__nexide.__dispatch`,
//! and assert the assembled [`ResponsePayload`] matches the expected
//! shape (status, headers, body).
//!
//! Pinned to `current_thread` + [`tokio::task::LocalSet`] because
//! [`V8Engine`] is `!Send`.

#![allow(clippy::future_not_send, clippy::significant_drop_tightening)]

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use nexide::engine::cjs::{FsResolver, ROOT_PARENT, default_registry};
use nexide::engine::{BootContext, V8Engine};
use nexide::ops::{HeaderPair, RequestMeta, RequestSlot};

/// JS handler installed at boot. Reads request meta + body, then
/// emits `pong:<body>` with two custom diagnostic headers
/// (`x-method`, `x-uri`).
const HANDLER: &str = r#"
const __nx = globalThis.__nexide;

__nx.__dispatch = function (idx, gen) {
  const meta = __nx.getMeta(idx, gen);

  const buf = new Uint8Array(64);
  const n = __nx.readBody(idx, gen, buf);
  const bodyView = buf.subarray(0, n);

  __nx.sendHead(idx, gen, 200, [
    ["content-type", "text/plain"],
    ["x-method", meta.method],
    ["x-uri", meta.uri],
  ]);

  const prefix = new Uint8Array([0x70, 0x6f, 0x6e, 0x67, 0x3a]); // "pong:"
  const out = new Uint8Array(prefix.length + bodyView.length);
  out.set(prefix, 0);
  out.set(bodyView, prefix.length);
  __nx.sendChunk(idx, gen, out);
  __nx.sendEnd(idx, gen);
};
"#;

#[tokio::test(flavor = "current_thread")]
async fn js_can_round_trip_request_to_response_via_ops() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let dir = tempfile::tempdir().expect("tempdir");
            let entry = dir.path().join("entry.cjs");
            std::fs::write(&entry, HANDLER).expect("write entry");

            let registry = Arc::new(default_registry().expect("registry"));
            let resolver = Arc::new(FsResolver::new(vec![dir.path().to_path_buf()], registry));
            let ctx = BootContext::new()
                .with_cjs(resolver)
                .with_cjs_root(ROOT_PARENT);
            let mut engine = V8Engine::boot_with(&entry, ctx).await.expect("boot");
            engine.start_pump(0).expect("start pump");

            let meta = RequestMeta::try_new("POST", "/api/echo").expect("meta");
            let headers = vec![HeaderPair {
                name: "content-type".to_owned(),
                value: "text/plain".to_owned(),
            }];
            let slot = RequestSlot::new(meta, headers, Bytes::from_static(b"ping"));
            let mut rx = engine.enqueue(slot);

            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            let payload = loop {
                engine.pump_once();
                match rx.try_recv() {
                    Ok(result) => break result.expect("handler succeeded"),
                    Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                        assert!(
                            std::time::Instant::now() <= deadline,
                            "handler did not complete within deadline",
                        );
                        tokio::time::sleep(Duration::from_millis(1)).await;
                    }
                    Err(other) => panic!("oneshot closed: {other:?}"),
                }
            };

            assert_eq!(payload.head.status, 200);
            assert_eq!(
                payload
                    .head
                    .headers
                    .iter()
                    .find(|(k, _)| k == "x-method")
                    .map(|(_, v)| v.as_str()),
                Some("POST"),
            );
            assert_eq!(
                payload
                    .head
                    .headers
                    .iter()
                    .find(|(k, _)| k == "x-uri")
                    .map(|(_, v)| v.as_str()),
                Some("/api/echo"),
            );
            assert_eq!(&payload.body[..], b"pong:ping");
            drop(dir);
        })
        .await;
}
