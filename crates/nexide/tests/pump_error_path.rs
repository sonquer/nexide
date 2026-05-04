//! Pump error path: synchronous throws and rejected promises in the
//! request handler must both settle the dispatcher's oneshot with
//! [`RequestFailure::Handler`] via `op_nexide_finish_error`. Without
//! that wiring the slot would leak until the worker was recycled.

#![allow(clippy::future_not_send, clippy::significant_drop_tightening)]

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use nexide::engine::cjs::{FsResolver, ROOT_PARENT, default_registry};
use nexide::engine::{BootContext, V8Engine};
use nexide::ops::{RequestFailure, RequestMeta, RequestSlot};

const HANDLER: &str = r"
const __nx = globalThis.__nexide;

__nx.__dispatch = function (idx, gen) {
  const meta = __nx.getMeta(idx, gen);
  const uri = meta[1];
  if (uri === '/sync-throw') {
    throw new Error('handler exploded synchronously');
  }
  if (uri === '/async-reject') {
    return Promise.reject(new Error('handler rejected'));
  }
  __nx.sendHead(idx, gen, 200, []);
  __nx.sendEnd(idx, gen);
};
";

fn slot_for(uri: &str) -> RequestSlot {
    let meta = RequestMeta::try_new("GET", uri).expect("valid meta");
    RequestSlot::new(meta, Vec::new(), Bytes::new())
}

async fn boot_handler() -> (V8Engine, tempfile::TempDir) {
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
    (engine, dir)
}

async fn drive_until<T>(engine: &mut V8Engine, rx: &mut tokio::sync::oneshot::Receiver<T>) -> T {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        engine.pump_once();
        if let Ok(payload) = rx.try_recv() {
            return payload;
        }
        assert!(
            std::time::Instant::now() <= deadline,
            "request did not settle within 5s",
        );
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn handler_sync_throw_routes_through_finish_error() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let (mut engine, _dir) = boot_handler().await;
            let mut rx = engine.enqueue(slot_for("/sync-throw"));
            let result = drive_until(&mut engine, &mut rx).await;
            let failure = result.expect_err("sync throw must surface as RequestFailure");
            match failure {
                RequestFailure::Handler(msg) => {
                    assert!(
                        msg.contains("handler exploded"),
                        "expected handler error text in failure, got {msg:?}"
                    );
                }
                other => panic!("expected Handler failure, got {other:?}"),
            }
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn handler_async_reject_routes_through_finish_error() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let (mut engine, _dir) = boot_handler().await;
            let mut rx = engine.enqueue(slot_for("/async-reject"));
            let result = drive_until(&mut engine, &mut rx).await;
            let failure = result.expect_err("async reject must surface as RequestFailure");
            match failure {
                RequestFailure::Handler(msg) => {
                    assert!(
                        msg.contains("handler rejected"),
                        "expected rejection text in failure, got {msg:?}"
                    );
                }
                other => panic!("expected Handler failure, got {other:?}"),
            }
        })
        .await;
}
