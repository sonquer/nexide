//! Integration tests for the `node:dns` polyfill backed by real
//! host-side hickory ops.
//!
//! Each test boots a fresh engine, runs a small CommonJS entrypoint
//! that exercises a piece of the `node:dns` surface, and treats a
//! thrown JS error as a Rust test failure. The `localhost` host is
//! always assumed to be resolvable on developer machines and CI.

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
async fn dns_promises_lookup_localhost_returns_loopback() {
    assert_passes(
        "const dns = require('node:dns').promises;\n\
         (async () => {\n\
           const r = await dns.lookup('localhost');\n\
           if (typeof r.address !== 'string') throw new Error('no address');\n\
           if (r.family !== 4 && r.family !== 6) throw new Error('bad family ' + r.family);\n\
         })().catch((e) => { throw e; });\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn dns_lookup_callback_form_returns_address() {
    assert_passes(
        "const dns = require('node:dns');\n\
         dns.lookup('localhost', (err, address, family) => {\n\
           if (err) throw err;\n\
           if (typeof address !== 'string') throw new Error('no address');\n\
           if (family !== 4 && family !== 6) throw new Error('bad family');\n\
         });\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn dns_lookup_all_returns_array() {
    assert_passes(
        "const dns = require('node:dns').promises;\n\
         (async () => {\n\
           const arr = await dns.lookup('localhost', { all: true });\n\
           if (!Array.isArray(arr) || arr.length === 0) throw new Error('expected array');\n\
           if (typeof arr[0].address !== 'string') throw new Error('bad shape');\n\
         })().catch((e) => { throw e; });\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn dns_promises_module_alias_works() {
    assert_passes(
        "const dns = require('node:dns/promises');\n\
         (async () => {\n\
           const r = await dns.lookup('localhost');\n\
           if (typeof r.address !== 'string') throw new Error('no address');\n\
         })().catch((e) => { throw e; });\n",
    )
    .await;
}
