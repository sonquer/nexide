//! Integration tests for the `node:net` polyfill backed by host-side
//! `tokio::net` ops.

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
async fn tcp_echo_round_trip() {
    assert_passes(
        "const net = require('node:net');\n\
         const server = net.createServer((sock) => {\n\
           sock.on('data', (chunk) => sock.write(chunk));\n\
         });\n\
         server.listen(0, '127.0.0.1', () => {\n\
           const port = server.address().port;\n\
           const client = net.createConnection({ host: '127.0.0.1', port });\n\
           client.on('connect', () => client.write('hello'));\n\
           client.on('data', (chunk) => {\n\
             if (chunk.toString('utf8') !== 'hello') {\n\
               throw new Error('bad echo: ' + chunk.toString('utf8'));\n\
             }\n\
             client.end();\n\
             server.close();\n\
           });\n\
         });\n\
         setTimeout(() => {\n\
           if (server._id) throw new Error('server still listening');\n\
         }, 100);\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn isip_helpers_match_node_semantics() {
    assert_passes(
        "const net = require('node:net');\n\
         if (net.isIP('127.0.0.1') !== 4) throw new Error('v4 fail');\n\
         if (net.isIP('::1') !== 6) throw new Error('v6 fail');\n\
         if (net.isIP('not-an-ip') !== 0) throw new Error('invalid fail');\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn connect_to_closed_port_emits_error() {
    assert_passes(
        "const net = require('node:net');\n\
         const sock = net.createConnection({ host: '127.0.0.1', port: 1 });\n\
         let saw = false;\n\
         sock.on('error', (err) => { saw = true; if (!err.code) throw new Error('no code'); });\n\
         setTimeout(() => { if (!saw) throw new Error('expected error'); }, 100);\n",
    )
    .await;
}
