//! Integration tests for the I/O `node:*` modules: `fs` (sandboxed),
//! `crypto`, `zlib`, and `stream`.
//!
//! Each test boots a real `V8Engine` via the V8 op bridge with a
//! `BootContext` carrying:
//!
//! * a CJS resolver pinned to the test's temp directory (so
//!   `globalThis.require` is wired and the entrypoint is loaded
//!   through `__nexideCjs.load`), and
//! * a sandboxed [`FsHandle`] over the same directory (so
//!   `op_fs_*` enforces `EACCES` for any path that escapes the
//!   sandbox).
//!
//! Test scripts run their assertions inline at module top level and
//! `throw` on failure. Boot returns the propagated `EngineError::JsRuntime`
//! whose message is asserted against the expected outcome.

#![allow(clippy::significant_drop_tightening, clippy::future_not_send)]

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use nexide::engine::cjs::{FsResolver, default_registry};
use nexide::engine::{BootContext, V8Engine};
use nexide::ops::FsHandle;
use tempfile::TempDir;

/// Builds a CJS test entrypoint inside a fresh temp directory.
///
/// `body` is wrapped in a top-level `try` so any thrown assertion
/// surfaces as a deterministic `EngineError::JsRuntime` whose message
/// contains the original failure text.
fn write_entry(body: &str) -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let entry = dir.path().join("entry.cjs");
    let mut file = std::fs::File::create(&entry).expect("create entry");
    file.write_all(body.as_bytes()).expect("write entry");
    file.flush().expect("flush entry");
    (dir, entry)
}

/// Boots the engine with a CJS-rooted sandbox over `dir` and the test
/// entrypoint at `entry`. Returns `Ok(())` when the script ran to
/// completion, `Err(message)` otherwise.
async fn run_in_sandbox(dir: &std::path::Path, entry: &std::path::Path) -> Result<(), String> {
    let registry = Arc::new(default_registry().map_err(|e| e.to_string())?);
    let resolver = Arc::new(FsResolver::new(vec![dir.to_path_buf()], registry));
    let fs = FsHandle::real(vec![dir.to_path_buf()]);
    let ctx = BootContext::new().with_cjs(resolver).with_fs(fs);
    V8Engine::boot_with(entry, ctx)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Convenience wrapper: build a CJS module from `body`, boot it, and
/// assert it runs to completion. Panics with the JS error otherwise.
async fn assert_passes(body: &str) {
    let (dir, entry) = write_entry(body);
    let dir_path = dir.path().to_path_buf();
    let local = tokio::task::LocalSet::new();
    let result = local
        .run_until(async move { run_in_sandbox(&dir_path, &entry).await })
        .await;
    if let Err(err) = result {
        panic!("test module failed: {err}");
    }
    drop(dir);
}

// ──────────────────────────────────────────────────────────────────────
// node:fs
// ──────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn fs_read_write_round_trips_inside_sandbox() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dir_path = dir.path().to_path_buf();
    let sample = dir_path.join("hello.txt");
    std::fs::write(&sample, b"world").expect("seed");
    let written = dir_path.join("written.txt");
    let entry = dir_path.join("entry.cjs");
    let body = format!(
        "const fs = require('node:fs');\n\
         const a = fs.readFileSync({sp:?}, 'utf8');\n\
         fs.writeFileSync({wp:?}, 'second');\n\
         const b = fs.readFileSync({wp:?}, 'utf8');\n\
         if (a + '|' + b !== 'world|second') {{\n\
           throw new Error('round-trip mismatch: ' + a + '|' + b);\n\
         }}\n",
        sp = sample.to_string_lossy(),
        wp = written.to_string_lossy(),
    );
    std::fs::write(&entry, body).expect("write entry");
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            run_in_sandbox(&dir_path, &entry)
                .await
                .expect("module passes");
        })
        .await;
    drop(dir);
}

#[tokio::test(flavor = "current_thread")]
async fn fs_blocks_path_outside_sandbox_with_eacces() {
    let dir = tempfile::tempdir().expect("tempdir");
    let elsewhere = tempfile::tempdir().expect("other");
    let secret = elsewhere.path().join("secret.txt");
    std::fs::write(&secret, b"shhh").expect("seed");
    let entry = dir.path().join("entry.cjs");
    let body = format!(
        "const fs = require('node:fs');\n\
         try {{\n\
           fs.readFileSync({p:?}, 'utf8');\n\
           throw new Error('LEAKED: read succeeded for path outside sandbox');\n\
         }} catch (err) {{\n\
           if ((err && err.code) !== 'EACCES') {{\n\
             throw new Error('expected EACCES, got ' + (err && err.code) + ': ' + err.message);\n\
           }}\n\
         }}\n",
        p = secret.to_string_lossy(),
    );
    std::fs::write(&entry, body).expect("write entry");
    let dir_path = dir.path().to_path_buf();
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            run_in_sandbox(&dir_path, &entry)
                .await
                .expect("module passes");
        })
        .await;
    drop(dir);
    drop(elsewhere);
}

#[tokio::test(flavor = "current_thread")]
async fn fs_readdir_lists_entries_and_stat_classifies_them() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dir_path = dir.path().to_path_buf();
    std::fs::create_dir(dir_path.join("sub")).expect("subdir");
    std::fs::write(dir_path.join("f.txt"), b"x").expect("file");
    let entry = dir_path.join("entry.cjs");
    let body = format!(
        "const fs = require('node:fs');\n\
         const list = fs.readdirSync({p:?}).sort();\n\
         const s_file = fs.statSync({fp:?});\n\
         const s_dir = fs.statSync({dp:?});\n\
         if (list.join(',') !== 'entry.cjs,f.txt,sub') {{\n\
           throw new Error('readdir mismatch: ' + list.join(','));\n\
         }}\n\
         if (!s_file.isFile()) throw new Error('expected file');\n\
         if (!s_dir.isDirectory()) throw new Error('expected dir');\n",
        p = dir_path.to_string_lossy(),
        fp = dir_path.join("f.txt").to_string_lossy(),
        dp = dir_path.join("sub").to_string_lossy(),
    );
    std::fs::write(&entry, body).expect("write entry");
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            run_in_sandbox(&dir_path, &entry)
                .await
                .expect("module passes");
        })
        .await;
    drop(dir);
}

// ──────────────────────────────────────────────────────────────────────
// node:crypto
// ──────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn crypto_hash_matches_well_known_sha256_vector() {
    assert_passes(
        "const c = require('node:crypto');\n\
         const got = c.createHash('sha256').update('abc').digest('hex');\n\
         const want = 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad';\n\
         if (got !== want) throw new Error('sha256 mismatch: ' + got);\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn crypto_hmac_round_trips_known_vector() {
    assert_passes(
        "const c = require('node:crypto');\n\
         const got = c.createHmac('sha256', 'key')\n\
           .update('The quick brown fox jumps over the lazy dog')\n\
           .digest('hex');\n\
         const want = 'f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8';\n\
         if (got !== want) throw new Error('hmac mismatch: ' + got);\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn crypto_random_uuid_matches_v4_layout() {
    assert_passes(
        "const c = require('node:crypto');\n\
         const u = c.randomUUID();\n\
         if (typeof u !== 'string' || u.length !== 36) throw new Error('len: ' + u);\n\
         if (u[14] !== '4') throw new Error('version: ' + u);\n\
         if (!'89ab'.includes(u[19])) throw new Error('variant: ' + u);\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn aes_gcm_seal_open_round_trip_and_tamper_fails() {
    assert_passes(
        "const c = require('node:crypto');\n\
         const key = c.randomBytes(32);\n\
         const iv = c.randomBytes(12);\n\
         const enc = c.createCipheriv('aes-256-gcm', key, iv);\n\
         enc.setAAD(Buffer.from('aad'));\n\
         enc.update(Buffer.from('top-secret-payload'));\n\
         const ct = enc.final();\n\
         const tag = enc.getAuthTag();\n\
         const dec = c.createDecipheriv('aes-256-gcm', key, iv);\n\
         dec.setAAD(Buffer.from('aad'));\n\
         dec.setAuthTag(tag);\n\
         dec.update(ct);\n\
         const pt = dec.final('utf8');\n\
         if (pt !== 'top-secret-payload') throw new Error('round-trip failed: ' + pt);\n\
         const bad = c.createDecipheriv('aes-256-gcm', key, iv);\n\
         bad.setAAD(Buffer.from('different'));\n\
         bad.setAuthTag(tag);\n\
         bad.update(ct);\n\
         let tampered_detected = false;\n\
         try {\n\
           bad.final();\n\
         } catch (_e) {\n\
           tampered_detected = true;\n\
         }\n\
         if (!tampered_detected) throw new Error('tamper not detected');\n",
    )
    .await;
}

// ──────────────────────────────────────────────────────────────────────
// node:zlib
// ──────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn zlib_gzip_round_trips_payload() {
    assert_passes(
        "const z = require('node:zlib');\n\
         const src = Buffer.from('hello-zlib-'.repeat(50));\n\
         const enc = z.gzipSync(src);\n\
         const dec = z.gunzipSync(enc);\n\
         if (!src.equals(dec)) throw new Error('gzip round-trip mismatch');\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn zlib_brotli_round_trips_payload() {
    assert_passes(
        "const z = require('node:zlib');\n\
         const src = Buffer.from('brotli content '.repeat(40));\n\
         const enc = z.brotliCompressSync(src);\n\
         const dec = z.brotliDecompressSync(enc);\n\
         if (!src.equals(dec)) throw new Error('brotli round-trip mismatch');\n",
    )
    .await;
}

// ──────────────────────────────────────────────────────────────────────
// node:stream
// ──────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn stream_readable_from_iterable_collects_chunks() {
    assert_passes(
        "const s = require('node:stream');\n\
         (async () => {\n\
           const r = s.Readable.from(['a', 'b', 'c']);\n\
           const out = [];\n\
           for await (const chunk of r) out.push(chunk);\n\
           if (out.join('') !== 'abc') throw new Error('stream mismatch: ' + out.join(','));\n\
         })().catch((err) => { throw err; });\n",
    )
    .await;
}
