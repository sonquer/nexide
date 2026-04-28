//! Integration tests for the `CommonJS` loader exposed via the V8
//! `__nexideCjs.load` bridge.
//!
//! Each test writes a tiny CJS fixture into a fresh tempdir and boots
//! a [`V8Engine`] with a [`FsResolver`] scoped to that directory.
//! Assertions live inline in the entry module; failures bubble out as
//! `EngineError::JsRuntime` and panic the test.

#![allow(clippy::future_not_send, clippy::significant_drop_tightening)]

use std::path::Path;
use std::sync::Arc;

use nexide::engine::cjs::{BuiltinModule, BuiltinRegistry, FsResolver, default_registry};
use nexide::engine::{BootContext, V8Engine};

/// In-memory builtin used by the registry test below.
struct Marker;
impl BuiltinModule for Marker {
    fn name(&self) -> &'static str {
        "marker"
    }
    fn source(&self) -> &'static str {
        "module.exports = { hello: (n) => 'hi ' + n };"
    }
}

/// Builds a registry pre-populated with the production node:* builtins
/// plus the test-only `node:marker` module.
fn registry_with_marker() -> Arc<BuiltinRegistry> {
    let mut reg = default_registry().expect("default registry");
    reg.register(Arc::new(Marker)).expect("register marker");
    Arc::new(reg)
}

/// Boots the engine with a CJS resolver pinned to `dir` and runs `entry`.
async fn run_module(dir: &Path, entry: &Path) -> Result<(), String> {
    let registry = registry_with_marker();
    let resolver = Arc::new(FsResolver::new(vec![dir.to_path_buf()], registry));
    let ctx = BootContext::new().with_cjs(resolver);
    V8Engine::boot_with(entry, ctx)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Drives `body` as the contents of an `entry.cjs` file inside a fresh
/// tempdir under `LocalSet`. Panics with the JS error if boot fails.
async fn assert_passes(extra_files: &[(&str, &str)], body: &str) {
    let dir = tempfile::tempdir().expect("tempdir");
    for (name, src) in extra_files {
        std::fs::write(dir.path().join(name), src).expect("seed file");
    }
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
async fn require_loads_a_relative_commonjs_file() {
    assert_passes(
        &[("smoke.cjs", "module.exports = { hello: (n) => 'hi ' + n };")],
        "const m = require('./smoke.cjs');\n\
         if (m.hello('world') !== 'hi world') throw new Error('relative require failed');\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn require_loads_json_file() {
    assert_passes(
        &[("data.json", r#"{"x":42,"name":"nexide"}"#)],
        "const d = require('./data.json');\n\
         if (d.x !== 42 || d.name !== 'nexide') throw new Error('json: ' + JSON.stringify(d));\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn require_resolves_node_builtin_from_registry() {
    assert_passes(
        &[],
        "const m = require('node:marker');\n\
         if (m.hello('builtin') !== 'hi builtin') throw new Error('builtin marker failed');\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn require_caches_modules_by_specifier() {
    assert_passes(
        &[(
            "counter.cjs",
            "let n = 0; module.exports = { next: () => ++n };",
        )],
        "const a = require('./counter.cjs');\n\
         const b = require('./counter.cjs');\n\
         a.next(); a.next();\n\
         if (b.next() !== 3) throw new Error('cache miss');\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn require_handles_cyclic_dependencies() {
    assert_passes(
        &[
            (
                "cyc_a.cjs",
                "exports.fromA = 'A';\n\
                 const b = require('./cyc_b.cjs');\n\
                 exports.bSeenAEarly = b.aSeenEarly;\n",
            ),
            (
                "cyc_b.cjs",
                "const a = require('./cyc_a.cjs');\n\
                 exports.aSeenEarly = a.fromA === 'A';\n\
                 exports.fromB = 'B';\n",
            ),
        ],
        "const a = require('./cyc_a.cjs');\n\
         if (a.fromA !== 'A' || a.bSeenAEarly !== true) {\n\
           throw new Error('cycle: ' + JSON.stringify(a));\n\
         }\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn require_unknown_node_module_throws_module_not_found() {
    assert_passes(
        &[],
        "let caught = '';\n\
         try { require('node:nonexistent'); }\n\
         catch (e) { caught = (e && e.message) || String(e); }\n\
         if (!caught.includes('MODULE_NOT_FOUND')) {\n\
           throw new Error('expected MODULE_NOT_FOUND, got: ' + caught);\n\
         }\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn require_exposes_dirname_and_filename() {
    assert_passes(
        &[("meta.cjs", "module.exports = { d: __dirname, f: __filename };")],
        "const m = require('./meta.cjs');\n\
         if (!m.f.endsWith('meta.cjs')) throw new Error('__filename: ' + m.f);\n\
         if (typeof m.d !== 'string' || m.d.length === 0) throw new Error('__dirname: ' + m.d);\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn require_resolves_alias_for_bare_node_specifier() {
    assert_passes(
        &[],
        "const a = require('node:path');\n\
         const b = require('path');\n\
         if (a !== b) throw new Error('node:path !== path');\n",
    )
    .await;
}
