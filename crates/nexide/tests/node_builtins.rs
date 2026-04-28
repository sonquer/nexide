//! Integration tests for the `node:*` builtin modules shipped via
//! [`default_registry`]: `path`, `querystring`, `url`, `util`, `os`,
//! `events`, plus alias resolution.

#![allow(clippy::future_not_send, clippy::significant_drop_tightening)]

use std::path::Path;
use std::sync::Arc;

use nexide::engine::cjs::{FsResolver, default_registry};
use nexide::engine::{BootContext, V8Engine};
use nexide::ops::{MapEnv, ProcessConfig};

/// Boots an engine with default node:* builtins, a deterministic
/// process config, and a live OS info source so `node:os` returns
/// real metadata.
async fn run_module(dir: &Path, entry: &Path) -> Result<(), String> {
    let registry = Arc::new(default_registry().map_err(|e| e.to_string())?);
    let resolver = Arc::new(FsResolver::new(vec![dir.to_path_buf()], registry));
    let env = Arc::new(MapEnv::from_pairs(std::iter::empty::<(String, String)>()));
    let process = ProcessConfig::builder(env).build();
    let ctx = BootContext::new()
        .with_cjs(resolver)
        .with_process(process);
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
async fn path_posix_join_and_parse_match_node_semantics() {
    assert_passes(
        "const path = require('node:path');\n\
         const aliased = require('path');\n\
         if (path !== aliased) throw new Error('alias mismatch');\n\
         const j = path.posix.join('/a', 'b', '..', 'c.txt');\n\
         if (j !== '/a/c.txt') throw new Error('join: ' + j);\n\
         const p = path.posix.parse('/foo/bar/baz.js');\n\
         if (p.dir !== '/foo/bar' || p.base !== 'baz.js' || p.ext !== '.js' || p.name !== 'baz') {\n\
           throw new Error('parse: ' + JSON.stringify(p));\n\
         }\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn querystring_round_trips_arrays_and_unicode() {
    assert_passes(
        "const qs = require('node:querystring');\n\
         const s = qs.stringify({ a: 1, b: ['x', 'y'], c: 'zażółć' });\n\
         const o = qs.parse(s);\n\
         if (o.a !== '1') throw new Error('a: ' + o.a);\n\
         if (!Array.isArray(o.b) || o.b[0] !== 'x' || o.b[1] !== 'y') throw new Error('b: ' + JSON.stringify(o.b));\n\
         if (o.c !== 'zażółć') throw new Error('c: ' + o.c);\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn url_module_exposes_url_class_and_legacy_helpers() {
    assert_passes(
        "const url = require('node:url');\n\
         if (typeof url.URL !== 'function') throw new Error('URL ctor missing');\n\
         const u = new url.URL('https://x.test/p?q=1#h');\n\
         if (u.hostname !== 'x.test') throw new Error('whatwg hostname');\n\
         const fp = url.fileURLToPath('file:///tmp/foo.txt');\n\
         if (fp !== '/tmp/foo.txt') throw new Error('fp: ' + fp);\n\
         const pu = url.pathToFileURL('/tmp/bar.txt');\n\
         if (!pu.href.startsWith('file:///tmp/bar')) throw new Error('pu: ' + pu.href);\n\
         const parsed = url.parse('https://x.test/p?q=1#h');\n\
         if (parsed.hostname !== 'x.test' || parsed.search !== '?q=1' || parsed.hash !== '#h') {\n\
           throw new Error('parsed: ' + JSON.stringify(parsed));\n\
         }\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn util_format_inspect_and_deep_strict_equal() {
    assert_passes(
        "const util = require('node:util');\n\
         const f = util.format('hello %s, %d', 'world', 7);\n\
         if (f !== 'hello world, 7') throw new Error('format: ' + f);\n\
         const ins = util.inspect({ a: 1, b: 'x' });\n\
         if (!ins.includes('a: 1')) throw new Error('inspect: ' + ins);\n\
         if (!util.isDeepStrictEqual({a:[1,2]},{a:[1,2]})) throw new Error('deep eq');\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn os_module_reports_host_metadata() {
    assert_passes(
        "const os = require('node:os');\n\
         if (typeof os.platform() !== 'string') throw new Error('platform');\n\
         if (typeof os.arch() !== 'string') throw new Error('arch');\n\
         if (typeof os.tmpdir() !== 'string') throw new Error('tmpdir');\n\
         if (typeof os.homedir() !== 'string') throw new Error('homedir');\n\
         if (typeof os.hostname() !== 'string') throw new Error('hostname');\n\
         if (!Array.isArray(os.cpus())) throw new Error('cpus');\n\
         if (os.cpus().length < 1) throw new Error('cpus.length');\n\
         if (os.endianness() !== 'LE' && os.endianness() !== 'BE') throw new Error('endian');\n\
         if (typeof os.uptime() !== 'number') throw new Error('uptime');\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn events_emitter_supports_on_once_off_and_count() {
    assert_passes(
        "const EventEmitter = require('node:events');\n\
         const ee = new EventEmitter();\n\
         const log = [];\n\
         const a = (x) => log.push('a:'+x);\n\
         const b = (x) => log.push('b:'+x);\n\
         ee.on('msg', a);\n\
         ee.once('msg', b);\n\
         ee.emit('msg', 1);\n\
         ee.emit('msg', 2);\n\
         ee.off('msg', a);\n\
         ee.emit('msg', 3);\n\
         if (ee.listenerCount('msg') !== 0) throw new Error('count: ' + ee.listenerCount('msg'));\n\
         if (log.join(',') !== 'a:1,b:1,a:2') throw new Error('order: ' + log.join(','));\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn require_resolves_node_prefix_and_bare_aliases() {
    assert_passes(
        "const a = require('node:events');\n\
         const b = require('events');\n\
         const c = require('node:path');\n\
         const d = require('path');\n\
         if (a !== b) throw new Error('events alias');\n\
         if (c !== d) throw new Error('path alias');\n",
    )
    .await;
}
