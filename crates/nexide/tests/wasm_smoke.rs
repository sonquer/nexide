//! WebAssembly smoke tests - the Prisma WASM query engine and other
//! WASM-based deps (e.g. `@libsql/client`, `argon2-browser`, the WASM
//! variant of `bcrypt`) all rely on V8's intrinsic `WebAssembly`
//! global. We don't ship a polyfill for it, so this file pins the
//! invariant that V8 in nexide builds with WASM enabled and exposes
//! the surface those deps need.

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
async fn webassembly_global_is_present() {
    assert_passes(
        "if (typeof WebAssembly !== 'object') throw new Error('WebAssembly missing');\n\
         for (const k of ['Module', 'Instance', 'Memory', 'Table', 'compile', 'instantiate', 'validate']) {\n\
           if (typeof WebAssembly[k] === 'undefined') {\n\
             throw new Error('WebAssembly.' + k + ' missing');\n\
           }\n\
         }\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn webassembly_validates_minimal_module() {
    // Smallest valid wasm module: 8-byte header.
    assert_passes(
        "const bytes = new Uint8Array([0,97,115,109,1,0,0,0]);\n\
         if (!WebAssembly.validate(bytes)) throw new Error('header rejected');\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn webassembly_compiles_and_runs_add_function() {
    // (module (func (export \"add\") (param i32 i32) (result i32) local.get 0 local.get 1 i32.add))
    // Hand-assembled binary - 41 bytes.
    assert_passes(
        "(async () => {\n\
           const bytes = new Uint8Array([\n\
             0x00,0x61,0x73,0x6d,0x01,0x00,0x00,0x00,\n\
             0x01,0x07,0x01,0x60,0x02,0x7f,0x7f,0x01,0x7f,\n\
             0x03,0x02,0x01,0x00,\n\
             0x07,0x07,0x01,0x03,0x61,0x64,0x64,0x00,0x00,\n\
             0x0a,0x09,0x01,0x07,0x00,0x20,0x00,0x20,0x01,0x6a,0x0b,\n\
           ]);\n\
           if (!WebAssembly.validate(bytes)) throw new Error('module invalid');\n\
           const mod = await WebAssembly.compile(bytes);\n\
           const inst = await WebAssembly.instantiate(mod);\n\
           const sum = inst.exports.add(20, 22);\n\
           if (sum !== 42) throw new Error('add returned ' + sum);\n\
         })().catch((err) => { throw err; });\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn webassembly_memory_is_usable_from_js() {
    // Confirms WebAssembly.Memory + .buffer exposure - this is the
    // pattern Prisma's WASM engine uses to copy strings in/out.
    assert_passes(
        "const mem = new WebAssembly.Memory({ initial: 1 });\n\
         if (!(mem.buffer instanceof ArrayBuffer)) throw new Error('memory.buffer not ArrayBuffer');\n\
         const view = new Uint8Array(mem.buffer);\n\
         view[0] = 0xab; view[1] = 0xcd;\n\
         if (view[0] !== 0xab || view[1] !== 0xcd) throw new Error('memory r/w');\n",
    )
    .await;
}
