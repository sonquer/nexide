//! Integration tests for the engine layer.
//!
//! These tests exercise the real `V8Engine` (V8) through the
//! [`IsolateHandle`] trait. Each test uses [`tempfile`] to lay down
//! a self-contained module tree on disk, so the suite is hermetic
//! and can run in parallel without crosstalk.

#![allow(clippy::significant_drop_tightening, clippy::manual_let_else, clippy::future_not_send)]

use std::io::Write;
use std::path::PathBuf;

use nexide::engine::{V8Engine, EngineError, IsolateHandle};
use tempfile::TempDir;

/// Builds a single-file ESM fixture under a fresh temporary directory.
///
/// Returns the directory guard (kept alive by the caller for the
/// lifetime of the test) plus the absolute path of the entrypoint.
fn write_module(filename: &str, source: &str) -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(filename);
    let mut file = std::fs::File::create(&path).expect("create module");
    file.write_all(source.as_bytes()).expect("write module");
    file.flush().expect("flush module");
    (dir, path)
}

#[tokio::test(flavor = "current_thread")]
async fn loads_trivial_esm() {
    let (_dir, entry) = write_module("entry.mjs", "export default 1;\n");

    let local = tokio::task::LocalSet::new();
    let result = local
        .run_until(async move { V8Engine::boot(&entry).await })
        .await;

    let engine = result.expect("trivial ESM should boot");
    let stats = engine.heap_stats();
    assert!(
        stats.heap_size_limit > 0,
        "real isolate must report a positive heap limit"
    );
    assert!(
        stats.utilization() >= 0.0 && stats.utilization() <= 1.0,
        "utilization out of bounds: {}",
        stats.utilization()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn reports_missing_module() {
    let dir = tempfile::tempdir().expect("tempdir");
    let entry = dir.path().join("does_not_exist.mjs");

    let local = tokio::task::LocalSet::new();
    let result = local
        .run_until(async move { V8Engine::boot(&entry).await })
        .await;
    let err = match result {
        Ok(_) => panic!("missing entrypoint must fail"),
        Err(err) => err,
    };

    match err {
        EngineError::ModuleResolution { path } => {
            assert!(path.ends_with("does_not_exist.mjs"));
        }
        other => panic!("expected ModuleResolution, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn propagates_js_exception() {
    let (_dir, entry) = write_module("boom.mjs", "throw new Error('boom from test');\n");

    let local = tokio::task::LocalSet::new();
    let result = local
        .run_until(async move { V8Engine::boot(&entry).await })
        .await;
    let err = match result {
        Ok(_) => panic!("throwing entrypoint must fail"),
        Err(err) => err,
    };

    match err {
        EngineError::JsRuntime { message } => {
            assert!(
                message.contains("boom from test"),
                "expected JS error text in message, got {message:?}"
            );
        }
        other => panic!("expected JsRuntime, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn pump_after_boot_is_a_noop_for_fully_evaluated_module() {
    let (_dir, entry) = write_module("idle.mjs", "export const v = 42;\n");

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let mut engine = V8Engine::boot(&entry).await.expect("boot");
            let before = engine.heap_stats();
            engine.pump().await.expect("pump should succeed");
            let after = engine.heap_stats();
            assert_eq!(
                before.heap_size_limit, after.heap_size_limit,
                "heap limit must remain stable across pumps"
            );
        })
        .await;
}
