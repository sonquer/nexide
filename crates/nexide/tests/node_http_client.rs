//! Integration tests for the `node:http` / `node:https` *client*
//! pipeline that runs through `op_http_request`.
//!
//! A minimal in-process HTTP/1.1 server backed by `tokio::net`
//! handles a single request and replies with a known body. The JS
//! script issues `http.request`, drains the response body, and
//! asserts on the round-trip.

#![allow(clippy::future_not_send, clippy::significant_drop_tightening)]

use std::path::Path;
use std::sync::Arc;

use nexide::engine::cjs::{FsResolver, default_registry};
use nexide::engine::{BootContext, V8Engine};
use nexide::ops::{MapEnv, ProcessConfig};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

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

async fn spawn_echo_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    tokio::task::spawn_local(async move {
        if let Ok((mut socket, _)) = listener.accept().await {
            let mut buf = [0u8; 4096];
            let _ = socket.read(&mut buf).await;
            let body = b"hello-from-rust";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len(),
            );
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.write_all(body).await;
            let _ = socket.shutdown().await;
        }
    });
    port
}

#[tokio::test(flavor = "current_thread")]
async fn http_client_round_trip() {
    let dir = tempfile::tempdir().expect("tempdir");
    let entry = dir.path().join("entry.cjs");
    let dir_path = dir.path().to_path_buf();
    let local = tokio::task::LocalSet::new();
    let result = local
        .run_until(async move {
            let port = spawn_echo_server().await;
            let script = format!(
                "const http = require('node:http');\n\
                 const req = http.request({{ host: '127.0.0.1', port: {port}, path: '/', method: 'GET' }}, (res) => {{\n\
                   if (res.statusCode !== 200) throw new Error('bad status: ' + res.statusCode);\n\
                   const chunks = [];\n\
                   res.on('data', (c) => chunks.push(c));\n\
                   res.on('end', () => {{\n\
                     const body = Buffer.concat(chunks).toString('utf8');\n\
                     if (body !== 'hello-from-rust') throw new Error('body mismatch: ' + body);\n\
                     globalThis.__test_ok = true;\n\
                   }});\n\
                 }});\n\
                 req.on('error', (e) => {{ throw e; }});\n\
                 req.end();\n\
                 setTimeout(() => {{ if (!globalThis.__test_ok) throw new Error('no response'); }}, 1500);\n",
            );
            std::fs::write(&entry, script).expect("write entry");
            run_module(&dir_path, &entry).await
        })
        .await;
    drop(dir);
    if let Err(err) = result {
        panic!("module failed: {err}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn http_request_to_closed_port_emits_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let entry = dir.path().join("entry.cjs");
    std::fs::write(
        &entry,
        "const http = require('node:http');\n\
         const req = http.request({ host: '127.0.0.1', port: 1, path: '/' });\n\
         let seen = false;\n\
         req.on('error', (err) => { seen = true; if (!err.code) throw new Error('no code'); });\n\
         req.end();\n\
         setTimeout(() => { if (!seen) throw new Error('expected error'); }, 500);\n",
    )
    .expect("write");
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
