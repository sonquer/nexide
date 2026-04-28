//! Integration tests for the Node-shaped global polyfills: `process`,
//! `Buffer`, `setImmediate`/`queueMicrotask`, and `URL`.
//!
//! Each test boots a [`V8Engine`] over a tiny CJS entry that runs
//! the assertions inline. Failures bubble out as `EngineError::JsRuntime`.
//! `process.env` gating is asserted via a deterministic [`MapEnv`] so
//! the suite never depends on the host's environment.

#![allow(clippy::future_not_send, clippy::significant_drop_tightening)]

use std::path::Path;
use std::sync::Arc;

use nexide::engine::cjs::{FsResolver, default_registry};
use nexide::engine::{BootContext, V8Engine};
use nexide::ops::{MapEnv, ProcessConfig};

/// Boots the engine with a CJS resolver pinned to `dir` plus a
/// deterministic [`ProcessConfig`] over `env_pairs`.
async fn run_with_env(dir: &Path, entry: &Path, env_pairs: &[(&str, &str)]) -> Result<(), String> {
    let registry = Arc::new(default_registry().map_err(|e| e.to_string())?);
    let resolver = Arc::new(FsResolver::new(vec![dir.to_path_buf()], registry));
    let env = Arc::new(MapEnv::from_pairs(
        env_pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned())),
    ));
    let process = ProcessConfig::builder(env).build();
    let ctx = BootContext::new().with_cjs(resolver).with_process(process);
    V8Engine::boot_with(entry, ctx)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Boots a one-shot CJS entrypoint with assertions inline.
async fn assert_passes(env: &[(&str, &str)], body: &str) {
    let dir = tempfile::tempdir().expect("tempdir");
    let entry = dir.path().join("entry.cjs");
    std::fs::write(&entry, body).expect("write entry");
    let dir_path = dir.path().to_path_buf();
    let env_owned: Vec<(String, String)> = env
        .iter()
        .map(|(k, v)| ((*k).into(), (*v).into()))
        .collect();
    let local = tokio::task::LocalSet::new();
    let result = local
        .run_until(async move {
            let env_refs: Vec<(&str, &str)> = env_owned
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            run_with_env(&dir_path, &entry, &env_refs).await
        })
        .await;
    drop(dir);
    if let Err(err) = result {
        panic!("module failed: {err}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn process_env_proxies_visible_keys() {
    assert_passes(
        &[
            ("NEXT_PUBLIC_FOO", "bar"),
            ("NODE_ENV", "production"),
            ("SECRET", "shh"),
        ],
        "if (process.env.NEXT_PUBLIC_FOO !== 'bar') throw new Error('public: ' + process.env.NEXT_PUBLIC_FOO);\n\
         if (process.env.NODE_ENV !== 'production') throw new Error('node_env: ' + process.env.NODE_ENV);\n\
         if (process.env.SECRET !== undefined) throw new Error('SECRET leaked: ' + process.env.SECRET);\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn process_meta_reports_platform_and_pid() {
    assert_passes(
        &[],
        "if (typeof process.platform !== 'string') throw new Error('platform missing');\n\
         if (typeof process.arch !== 'string') throw new Error('arch missing');\n\
         if (typeof process.pid !== 'number') throw new Error('pid missing');\n\
         if (typeof process.cwd() !== 'string') throw new Error('cwd missing');\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn process_hrtime_bigint_is_monotonic() {
    assert_passes(
        &[],
        "const a = process.hrtime.bigint();\n\
         let spin = 0;\n\
         for (let i = 0; i < 100000; i++) spin += i;\n\
         const b = process.hrtime.bigint();\n\
         if (b <= a) throw new Error('hrtime not monotonic');\n\
         if (spin === 0) throw new Error('spin discarded');\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn buffer_round_trips_utf8_base64_and_hex() {
    assert_passes(
        &[],
        "const a = Buffer.from('hello').toString('base64');\n\
         if (a !== 'aGVsbG8=') throw new Error('base64: ' + a);\n\
         const b = Buffer.from(a, 'base64').toString();\n\
         if (b !== 'hello') throw new Error('decode: ' + b);\n\
         const c = Buffer.from('hi').toString('hex');\n\
         if (c !== '6869') throw new Error('hex: ' + c);\n\
         const d = Buffer.from(c, 'hex').toString();\n\
         if (d !== 'hi') throw new Error('hex-decode: ' + d);\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn buffer_concat_alloc_and_byte_length() {
    assert_passes(
        &[],
        "const x = Buffer.from('foo');\n\
         const y = Buffer.from('bar');\n\
         const z = Buffer.concat([x, y]);\n\
         if (z.toString() !== 'foobar') throw new Error('concat: ' + z.toString());\n\
         if (z.length !== 6) throw new Error('len: ' + z.length);\n\
         if (Buffer.byteLength('héllo') !== 6) throw new Error('byteLength: ' + Buffer.byteLength('héllo'));\n\
         const filled = Buffer.alloc(4, 'a');\n\
         if (filled.toString() !== 'aaaa') throw new Error('alloc: ' + filled.toString());\n\
         if (!Buffer.isBuffer(z)) throw new Error('isBuffer false');\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn buffer_uint_readers_and_writers_round_trip() {
    assert_passes(
        &[],
        "const buf = Buffer.alloc(8);\n\
         buf.writeUInt32BE(0xdeadbeef, 0);\n\
         buf.writeUInt32LE(0x11223344, 4);\n\
         const be = buf.readUInt32BE(0);\n\
         const le = buf.readUInt32LE(4);\n\
         if (be.toString(16) !== 'deadbeef') throw new Error('be: ' + be.toString(16));\n\
         if (le.toString(16) !== '11223344') throw new Error('le: ' + le.toString(16));\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn queue_microtask_runs_before_top_level_completes() {
    assert_passes(
        &[],
        "let order = [];\n\
         queueMicrotask(() => order.push('mt'));\n\
         Promise.resolve().then(() => order.push('p'));\n\
         globalThis.__verify = () => {\n\
           if (order.join(',') !== 'mt,p') throw new Error('order: ' + order.join(','));\n\
         };\n\
         Promise.resolve().then(() => globalThis.__verify());\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn url_and_url_search_params_are_global() {
    assert_passes(
        &[],
        "const u = new URL('https://x.test/p?a=1&b=2#h');\n\
         if (u.hostname !== 'x.test') throw new Error('hostname: ' + u.hostname);\n\
         if (u.searchParams.get('a') !== '1') throw new Error('a: ' + u.searchParams.get('a'));\n\
         const sp = new URLSearchParams('x=1&y=2');\n\
         if (sp.get('y') !== '2') throw new Error('y: ' + sp.get('y'));\n",
    )
    .await;
}
