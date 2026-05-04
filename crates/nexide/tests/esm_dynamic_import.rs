//! Integration tests for real ESM dynamic-import support.
//!
//! Each test seeds a tempdir, boots a [`V8Engine`] via the CJS entry
//! path (matching production), and exercises `await import(...)` to
//! make sure pure-ESM packages, ESM-imports-CJS, and bare-specifier
//! resolution through `node_modules` all work.

#![allow(clippy::future_not_send, clippy::significant_drop_tightening)]

use std::path::Path;
use std::sync::Arc;

use nexide::engine::cjs::{FsResolver, default_registry};
use nexide::engine::{BootContext, V8Engine};

async fn run_module(dir: &Path, entry: &Path) -> Result<(), String> {
    let registry = Arc::new(default_registry().expect("default registry"));
    let resolver = Arc::new(FsResolver::new(vec![dir.to_path_buf()], registry));
    let ctx = BootContext::new().with_cjs(resolver);
    V8Engine::boot_with(entry, ctx)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

async fn assert_passes(extra_files: &[(&str, &str)], body: &str) {
    let dir = tempfile::tempdir().expect("tempdir");
    for (name, src) in extra_files {
        let target = dir.path().join(name);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).expect("create parent");
        }
        std::fs::write(&target, src).expect("seed file");
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

/// Confirms `await import('./esm-mod.mjs')` exposes both directly
/// declared and re-exported bindings on the namespace.
#[tokio::test(flavor = "current_thread")]
async fn esm_dynamic_import_relative_mjs_with_reexport() {
    assert_passes(
        &[
            (
                "util.mjs",
                "export const helper = (n) => 'h:' + n;\n\
                 export const VERSION = 7;\n",
            ),
            (
                "esm-mod.mjs",
                "export const value = 42;\n\
                 export { helper, VERSION } from './util.mjs';\n",
            ),
        ],
        "(async () => {\n\
           const ns = await import('./esm-mod.mjs');\n\
           if (ns.value !== 42) throw new Error('value=' + ns.value);\n\
           if (ns.VERSION !== 7) throw new Error('VERSION=' + ns.VERSION);\n\
           if (ns.helper('x') !== 'h:x') throw new Error('helper');\n\
           globalThis.__nexide_esm_marker = true;\n\
         })().catch((e) => { throw e; });\n",
    )
    .await;
}

/// ESM-side dynamic import of a CJS file: namespace must have
/// `default = module.exports` and named props mirroring enumerable
/// own keys.
#[tokio::test(flavor = "current_thread")]
async fn esm_dynamic_import_cjs_pkg() {
    assert_passes(
        &[(
            "pkg-cjs/index.js",
            "module.exports = { foo: 1, bar: 'b' };\n",
        )],
        "(async () => {\n\
           const ns = await import('./pkg-cjs/index.js');\n\
           if (!ns.default) throw new Error('default missing');\n\
           if (ns.default.foo !== 1) throw new Error('default.foo');\n\
           if (ns.foo !== 1) throw new Error('foo on ns: ' + ns.foo);\n\
           if (ns.bar !== 'b') throw new Error('bar on ns');\n\
         })().catch((e) => { throw e; });\n",
    )
    .await;
}

/// Bare specifier into a node_modules-installed pure-ESM package
/// (`type: module`, only `import` condition exposed).
#[tokio::test(flavor = "current_thread")]
async fn esm_dynamic_import_bare_node_modules_package() {
    assert_passes(
        &[
            (
                "node_modules/tinypkg/package.json",
                "{\n\
                   \"name\": \"tinypkg\",\n\
                   \"type\": \"module\",\n\
                   \"main\": \"./index.mjs\",\n\
                   \"exports\": {\n\
                     \".\": { \"import\": \"./index.mjs\", \"default\": \"./index.mjs\" }\n\
                   }\n\
                 }\n",
            ),
            (
                "node_modules/tinypkg/index.mjs",
                "export const tinypkgValue = 'tiny-' + 1;\n\
                 export default { kind: 'tinypkg' };\n",
            ),
        ],
        "(async () => {\n\
           const ns = await import('tinypkg');\n\
           if (ns.tinypkgValue !== 'tiny-1') {\n\
             throw new Error('tinypkgValue=' + ns.tinypkgValue);\n\
           }\n\
           if (!ns.default || ns.default.kind !== 'tinypkg') {\n\
             throw new Error('default missing');\n\
           }\n\
         })().catch((e) => { throw e; });\n",
    )
    .await;
}

/// ESM module that statically imports a sibling ESM AND a CJS file,
/// invoked through `await import(...)` from the CJS root entry.
#[tokio::test(flavor = "current_thread")]
async fn esm_static_imports_mix_of_esm_and_cjs() {
    assert_passes(
        &[
            ("util.mjs", "export const tag = (n) => 'T<' + n + '>';\n"),
            ("legacy.cjs", "module.exports = { legacy: 'yes' };\n"),
            (
                "root.mjs",
                "import { tag } from './util.mjs';\n\
                 import legacy from './legacy.cjs';\n\
                 export const result = tag(legacy.legacy);\n",
            ),
        ],
        "(async () => {\n\
           const ns = await import('./root.mjs');\n\
           if (ns.result !== 'T<yes>') throw new Error('result=' + ns.result);\n\
         })().catch((e) => { throw e; });\n",
    )
    .await;
}
