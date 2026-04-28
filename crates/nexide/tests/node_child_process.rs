//! Integration tests for `node:child_process` backed by host
//! `tokio::process` ops.

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
#[cfg(unix)]
async fn spawn_echo_captures_stdout() {
    assert_passes(
        "const { spawn } = require('node:child_process');\n\
         const child = spawn('/bin/echo', ['hello-from-child']);\n\
         const chunks = [];\n\
         child.stdout.on('data', (c) => chunks.push(c));\n\
         child.on('close', (code) => {\n\
           if (code !== 0) throw new Error('bad code: ' + code);\n\
           const out = Buffer.concat(chunks).toString('utf8').trim();\n\
           if (out !== 'hello-from-child') throw new Error('bad stdout: ' + out);\n\
           globalThis.__test_ok = true;\n\
         });\n\
         setTimeout(() => { if (!globalThis.__test_ok) throw new Error('no exit'); }, 2000);\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
#[cfg(unix)]
async fn exec_runs_through_shell() {
    assert_passes(
        "const { exec } = require('node:child_process');\n\
         exec('echo via-shell', (err, stdout, stderr) => {\n\
           if (err) throw err;\n\
           if (stdout.trim() !== 'via-shell') throw new Error('bad stdout: ' + stdout);\n\
           globalThis.__test_ok = true;\n\
         });\n\
         setTimeout(() => { if (!globalThis.__test_ok) throw new Error('no exit'); }, 2000);\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn spawn_missing_program_throws_enoent() {
    assert_passes(
        "const { spawn } = require('node:child_process');\n\
         let saw = false;\n\
         try {\n\
           spawn('/this/program/does/not/exist');\n\
         } catch (err) {\n\
           saw = true;\n\
           if (err.code !== 'ENOENT') throw new Error('bad code: ' + err.code);\n\
         }\n\
         if (!saw) throw new Error('expected error');\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn spawn_sync_throws_err_not_available() {
    assert_passes(
        "const { spawnSync } = require('node:child_process');\n\
         let saw = false;\n\
         try { spawnSync('echo'); } catch (err) {\n\
           saw = true;\n\
           if (err.code !== 'ERR_NOT_AVAILABLE') throw new Error('bad code: ' + err.code);\n\
         }\n\
         if (!saw) throw new Error('expected error');\n",
    )
    .await;
}
