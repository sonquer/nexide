//! Integration tests for the timer polyfill backed by the host-side
//! `op_timer_sleep` op.
//!
//! Each test boots a fresh engine, runs a CommonJS entrypoint that
//! exercises one piece of the timer surface, and treats a thrown JS
//! error as a Rust test failure. Delays are kept short (5–25 ms) so
//! the suite stays cheap enough for CI.

#![allow(clippy::future_not_send, clippy::significant_drop_tightening)]

use std::path::Path;
use std::sync::Arc;

use nexide::engine::cjs::{FsResolver, default_registry};
use nexide::engine::{BootContext, V8Engine};
use nexide::ops::{MapEnv, ProcessConfig};

async fn run_module(dir: &Path, entry: &Path) -> Result<(), String> {
    let registry = Arc::new(default_registry().map_err(|e| e.to_string())?);
    let resolver = Arc::new(FsResolver::new(vec![dir.to_path_buf()], registry));
    let env = Arc::new(MapEnv::from_pairs(std::iter::empty::<(String, String)>()));
    let process = ProcessConfig::builder(env).build();
    let ctx = BootContext::new().with_cjs(resolver).with_process(process);
    V8Engine::boot_with(entry, ctx)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

async fn assert_passes(body: &str) {
    let dir = tempfile::tempdir().expect("tempdir");
    let entry = dir.path().join("entry.cjs");
    std::fs::write(&entry, body).expect("write entry");
    let dir_path = dir.path().to_path_buf();
    let local = tokio::task::LocalSet::new();
    let result = local
        .run_until(async move { run_module(&dir_path, &entry).await })
        .await;
    drop(dir);
    if let Err(err) = result {
        panic!("module failed: {err}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn set_timeout_fires_after_delay() {
    assert_passes(
        "const start = Date.now();\n\
         setTimeout(() => {\n\
           const dt = Date.now() - start;\n\
           if (dt < 5) throw new Error('fired too early: ' + dt);\n\
         }, 10);\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn clear_timeout_cancels_callback() {
    assert_passes(
        "let fired = false;\n\
         const id = setTimeout(() => { fired = true; }, 5);\n\
         clearTimeout(id);\n\
         setTimeout(() => {\n\
           if (fired) throw new Error('cleared timeout still fired');\n\
         }, 25);\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn set_immediate_runs_after_microtasks() {
    assert_passes(
        "let order = [];\n\
         setImmediate(() => order.push('immediate'));\n\
         queueMicrotask(() => order.push('micro'));\n\
         setTimeout(() => {\n\
           if (order[0] !== 'micro' || order[1] !== 'immediate') {\n\
             throw new Error('bad order: ' + order.join(','));\n\
           }\n\
         }, 15);\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn set_interval_clears_after_two_ticks() {
    assert_passes(
        "let count = 0;\n\
         const id = setInterval(() => {\n\
           count++;\n\
           if (count >= 2) clearInterval(id);\n\
         }, 5);\n\
         setTimeout(() => {\n\
           if (count < 2) throw new Error('interval did not fire twice: ' + count);\n\
           if (count > 3) throw new Error('interval did not stop: ' + count);\n\
         }, 40);\n",
    )
    .await;
}
