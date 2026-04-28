//! Integration tests for `node:zlib` streaming Transform classes.

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
async fn deflate_inflate_round_trip() {
    assert_passes(
        "const zlib = require('node:zlib');\n\
         const payload = Buffer.from('hello-streaming-' + 'x'.repeat(64));\n\
         const enc = zlib.createDeflate();\n\
         const compressedChunks = [];\n\
         enc.on('data', (c) => compressedChunks.push(c));\n\
         enc.on('end', () => {\n\
           const compressed = Buffer.concat(compressedChunks);\n\
           const dec = zlib.createInflate();\n\
           const outChunks = [];\n\
           dec.on('data', (c) => outChunks.push(c));\n\
           dec.on('end', () => {\n\
             const got = Buffer.concat(outChunks).toString('utf8');\n\
             if (got !== payload.toString('utf8')) throw new Error('mismatch: ' + got);\n\
             globalThis.__test_ok = true;\n\
           });\n\
           dec.end(compressed);\n\
         });\n\
         enc.end(payload);\n\
         setTimeout(() => { if (!globalThis.__test_ok) throw new Error('no end'); }, 1500);\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn gzip_gunzip_round_trip() {
    assert_passes(
        "const zlib = require('node:zlib');\n\
         const payload = Buffer.from('gzip-streaming-payload');\n\
         const enc = zlib.createGzip();\n\
         const chunks = [];\n\
         enc.on('data', (c) => chunks.push(c));\n\
         enc.on('end', () => {\n\
           const compressed = Buffer.concat(chunks);\n\
           const dec = zlib.createGunzip();\n\
           const outChunks = [];\n\
           dec.on('data', (c) => outChunks.push(c));\n\
           dec.on('end', () => {\n\
             const got = Buffer.concat(outChunks).toString('utf8');\n\
             if (got !== payload.toString('utf8')) throw new Error('mismatch: ' + got);\n\
             globalThis.__test_ok = true;\n\
           });\n\
           dec.end(compressed);\n\
         });\n\
         enc.end(payload);\n\
         setTimeout(() => { if (!globalThis.__test_ok) throw new Error('no end'); }, 1500);\n",
    )
    .await;
}
