//! Integration tests for the `AsyncLocalStorage` polyfill.
//!
//! Each test boots a real `V8Engine` with the production polyfill
//! set, then asserts behaviour by executing a JavaScript assertion
//! script. Failures bubble up as `JsRuntime` errors with the assertion
//! text, which makes regressions self-describing.
//!
//! All tests run on a `LocalSet` because `V8Engine` is `!Send`.

#![allow(clippy::significant_drop_tightening, clippy::future_not_send)]

use std::io::Write;
use std::path::PathBuf;

use nexide::engine::{V8Engine, IsolateHandle};
use tempfile::TempDir;

/// Writes a JavaScript module under a fresh temporary directory and
/// returns the directory guard plus the absolute entrypoint path.
fn write_module(filename: &str, source: &str) -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(filename);
    let mut file = std::fs::File::create(&path).expect("create module");
    file.write_all(source.as_bytes()).expect("write module");
    file.flush().expect("flush module");
    (dir, path)
}

/// Boots the engine on the supplied module and asserts that booting
/// completes successfully — any thrown error from the module body
/// (typically an assertion failure) is treated as a test failure with
/// a descriptive message.
async fn run_assertion_module(source: &str) {
    let (_dir, entry) = write_module("test.mjs", source);
    let local = tokio::task::LocalSet::new();
    let result = local
        .run_until(async move { V8Engine::boot(&entry).await })
        .await;
    if let Err(err) = result {
        panic!("assertion module failed: {err}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn propagates_through_microtask_await() {
    run_assertion_module(
        r"
        const als = new AsyncLocalStorage();
        await als.run({ id: 42 }, async () => {
            await Promise.resolve();
            await Promise.resolve();
            const got = als.getStore();
            if (!got || got.id !== 42) {
                throw new Error('expected id=42 across awaits, got ' + JSON.stringify(got));
            }
        });
        ",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn propagates_through_chained_microtasks() {
    run_assertion_module(
        r"
        const als = new AsyncLocalStorage();
        await als.run({ tag: 'outer' }, async () => {
            const value = await Promise.resolve()
                .then(() => Promise.resolve())
                .then(() => als.getStore());
            if (!value || value.tag !== 'outer') {
                throw new Error('lost context in chained microtasks: ' + JSON.stringify(value));
            }
        });
        ",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn nested_runs_use_independent_stores() {
    run_assertion_module(
        r"
        const als = new AsyncLocalStorage();
        await als.run({ level: 'outer' }, async () => {
            await als.run({ level: 'inner' }, async () => {
                await Promise.resolve();
                if (als.getStore().level !== 'inner') {
                    throw new Error('inner run did not see inner store');
                }
            });
            await Promise.resolve();
            if (als.getStore().level !== 'outer') {
                throw new Error('outer store was lost after inner run');
            }
        });
        ",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn get_store_outside_run_is_undefined() {
    run_assertion_module(
        r"
        const als = new AsyncLocalStorage();
        if (als.getStore() !== undefined) {
            throw new Error('store should be undefined outside run()');
        }
        await als.run({ x: 1 }, async () => { await Promise.resolve(); });
        if (als.getStore() !== undefined) {
            throw new Error('store leaked outside run()');
        }
        ",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn exception_inside_run_does_not_leak_context() {
    run_assertion_module(
        r"
        const als = new AsyncLocalStorage();
        try {
            await als.run({ tainted: true }, async () => {
                throw new Error('expected failure');
            });
        } catch (err) {
            if (!String(err.message).includes('expected failure')) {
                throw err;
            }
        }
        if (als.getStore() !== undefined) {
            throw new Error('context leaked after exception');
        }
        ",
    )
    .await;
}
