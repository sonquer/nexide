//! Integration tests for the new node builtin polyfills shipped in
//! support of Next.js standalone production deployments:
//!
//!  - node:assert/strict
//!  - node:util/types
//!  - node:path/posix, node:path/win32
//!  - node:stream/web, node:stream/promises, node:stream/consumers
//!  - node:diagnostics_channel
//!  - node:http2
//!  - node:readline, node:readline/promises

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

// ---------- assert/strict ----------

#[tokio::test(flavor = "current_thread")]
async fn assert_strict_is_alias_of_assert() {
    assert_passes(
        "const a = require('node:assert');\n\
         const s = require('node:assert/strict');\n\
         if (a !== s && a.strictEqual !== s.strictEqual) {\n\
           throw new Error('assert/strict should expose the same surface as assert');\n\
         }\n\
         s.strictEqual(1, 1);\n\
         let threw = false;\n\
         try { s.strictEqual(1, '1'); } catch { threw = true; }\n\
         if (!threw) throw new Error('strictEqual must distinguish 1 vs \"1\"');\n",
    )
    .await;
}

// ---------- util/types ----------

#[tokio::test(flavor = "current_thread")]
async fn util_types_basic_predicates() {
    assert_passes(
        "const t = require('node:util/types');\n\
         if (!t.isPromise(Promise.resolve())) throw new Error('isPromise');\n\
         if (!t.isMap(new Map())) throw new Error('isMap');\n\
         if (!t.isSet(new Set())) throw new Error('isSet');\n\
         if (!t.isDate(new Date())) throw new Error('isDate');\n\
         if (!t.isRegExp(/x/)) throw new Error('isRegExp');\n\
         if (!t.isUint8Array(new Uint8Array(1))) throw new Error('isUint8Array');\n\
         if (!t.isTypedArray(new Float32Array(1))) throw new Error('isTypedArray');\n\
         if (!t.isNativeError(new Error('x'))) throw new Error('isNativeError');\n\
         if (t.isPromise({})) throw new Error('isPromise false-positive');\n",
    )
    .await;
}

// ---------- path/posix + path/win32 ----------

#[tokio::test(flavor = "current_thread")]
async fn path_posix_uses_forward_slashes_regardless_of_host() {
    assert_passes(
        "const p = require('node:path/posix');\n\
         const j = p.join('a', 'b', 'c');\n\
         if (j !== 'a/b/c') throw new Error('posix.join: ' + j);\n\
         if (p.sep !== '/') throw new Error('posix.sep: ' + p.sep);\n\
         if (!p.isAbsolute('/x/y')) throw new Error('posix.isAbsolute');\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn path_win32_uses_backslashes_regardless_of_host() {
    assert_passes(
        "const p = require('node:path/win32');\n\
         const j = p.join('a', 'b', 'c');\n\
         if (j !== 'a\\\\b\\\\c') throw new Error('win32.join: ' + JSON.stringify(j));\n\
         if (p.sep !== '\\\\') throw new Error('win32.sep: ' + JSON.stringify(p.sep));\n",
    )
    .await;
}

// ---------- stream/web ----------

#[tokio::test(flavor = "current_thread")]
async fn stream_web_reexports_global_classes() {
    assert_passes(
        "const sw = require('node:stream/web');\n\
         if (typeof sw.ReadableStream !== 'function') throw new Error('ReadableStream missing');\n\
         if (typeof sw.TransformStream !== 'function') throw new Error('TransformStream missing');\n\
         if (typeof sw.WritableStream !== 'function') throw new Error('WritableStream missing');\n\
         // Identity check vs globalThis (when global exists).\n\
         if (typeof globalThis.ReadableStream === 'function'\n\
             && sw.ReadableStream !== globalThis.ReadableStream) {\n\
           throw new Error('stream/web ReadableStream should be the global one');\n\
         }\n",
    )
    .await;
}

// ---------- stream/promises ----------

#[tokio::test(flavor = "current_thread")]
async fn stream_promises_pipeline_finishes_passthrough() {
    assert_passes(
        "(async () => {\n\
           const { Readable, PassThrough } = require('node:stream');\n\
           const { pipeline, finished } = require('node:stream/promises');\n\
           const src = Readable.from(['a', 'b', 'c']);\n\
           const sink = new PassThrough();\n\
           const collected = [];\n\
           sink.on('data', (c) => collected.push(c));\n\
           await pipeline(src, sink);\n\
           await finished(sink);\n\
           if (collected.join('') !== 'abc') {\n\
             throw new Error('pipeline output: ' + collected.join(''));\n\
           }\n\
         })().catch((err) => { throw err; });\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn stream_promises_pipeline_rejects_callback() {
    assert_passes(
        "const { pipeline } = require('node:stream/promises');\n\
         let threw = false;\n\
         try { pipeline({}, () => {}); } catch (e) {\n\
           if (e instanceof TypeError) threw = true;\n\
         }\n\
         if (!threw) throw new Error('pipeline must reject callback form');\n",
    )
    .await;
}

// ---------- stream/consumers ----------

#[tokio::test(flavor = "current_thread")]
async fn stream_consumers_drain_node_readable() {
    assert_passes(
        "(async () => {\n\
           const { Readable } = require('node:stream');\n\
           const sc = require('node:stream/consumers');\n\
           const buf = await sc.buffer(Readable.from(['hello ', 'world']));\n\
           if (buf.toString('utf8') !== 'hello world') {\n\
             throw new Error('buffer: ' + buf.toString('utf8'));\n\
           }\n\
           const txt = await sc.text(Readable.from(['{\"a\":', '1}']));\n\
           if (txt !== '{\"a\":1}') throw new Error('text: ' + txt);\n\
           const j = await sc.json(Readable.from(['{\"a\":', '1}']));\n\
           if (j.a !== 1) throw new Error('json.a: ' + j.a);\n\
           const ab = await sc.arrayBuffer(Readable.from(['xyz']));\n\
           if (!(ab instanceof ArrayBuffer) || ab.byteLength !== 3) {\n\
             throw new Error('arrayBuffer wrong: ' + ab.byteLength);\n\
           }\n\
         })().catch((err) => { throw err; });\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn stream_consumers_drain_async_iterable() {
    assert_passes(
        "(async () => {\n\
           const sc = require('node:stream/consumers');\n\
           async function* gen() { yield 'foo'; yield 'bar'; }\n\
           const t = await sc.text(gen());\n\
           if (t !== 'foobar') throw new Error('asyncIterable text: ' + t);\n\
         })().catch((err) => { throw err; });\n",
    )
    .await;
}

// ---------- diagnostics_channel ----------

#[tokio::test(flavor = "current_thread")]
async fn diagnostics_channel_pubsub_roundtrip() {
    assert_passes(
        "const dc = require('node:diagnostics_channel');\n\
         const ch = dc.channel('nexide.test.basic');\n\
         if (ch.hasSubscribers) throw new Error('expected zero subs');\n\
         const seen = [];\n\
         const fn = (msg, name) => seen.push([name, msg.x]);\n\
         dc.subscribe('nexide.test.basic', fn);\n\
         if (!ch.hasSubscribers) throw new Error('expected one sub');\n\
         if (!dc.hasSubscribers('nexide.test.basic')) throw new Error('hasSubscribers');\n\
         ch.publish({ x: 42 });\n\
         ch.publish({ x: 7 });\n\
         dc.unsubscribe('nexide.test.basic', fn);\n\
         if (ch.hasSubscribers) throw new Error('unsubscribe failed');\n\
         ch.publish({ x: 999 });\n\
         if (seen.length !== 2) throw new Error('callbacks fired ' + seen.length);\n\
         if (seen[0][0] !== 'nexide.test.basic' || seen[0][1] !== 42) {\n\
           throw new Error('payload mismatch: ' + JSON.stringify(seen[0]));\n\
         }\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn diagnostics_channel_same_name_returns_same_instance() {
    assert_passes(
        "const dc = require('node:diagnostics_channel');\n\
         const a = dc.channel('nexide.dedup');\n\
         const b = dc.channel('nexide.dedup');\n\
         if (a !== b) throw new Error('channel() must memoise');\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn diagnostics_channel_tracing_sync_and_promise() {
    assert_passes(
        "(async () => {\n\
           const dc = require('node:diagnostics_channel');\n\
           const tc = dc.tracingChannel('nexide.trace');\n\
           const events = [];\n\
           tc.subscribe({\n\
             start: () => events.push('start'),\n\
             end: () => events.push('end'),\n\
             asyncStart: () => events.push('asyncStart'),\n\
             asyncEnd: () => events.push('asyncEnd'),\n\
             error: () => events.push('error'),\n\
           });\n\
           const r = tc.traceSync(() => 7, { op: 'sync' });\n\
           if (r !== 7) throw new Error('traceSync return');\n\
           await tc.tracePromise(async () => 'ok', { op: 'async' });\n\
           if (!events.includes('start') || !events.includes('end')\n\
               || !events.includes('asyncStart') || !events.includes('asyncEnd')) {\n\
             throw new Error('tracing events: ' + events.join(','));\n\
           }\n\
         })().catch((err) => { throw err; });\n",
    )
    .await;
}

// ---------- http2 ----------

#[tokio::test(flavor = "current_thread")]
async fn http2_loads_and_exposes_constants() {
    assert_passes(
        "const h2 = require('node:http2');\n\
         if (typeof h2.constants !== 'object') throw new Error('constants missing');\n\
         if (h2.constants.HTTP2_HEADER_PATH !== ':path') {\n\
           throw new Error('HTTP2_HEADER_PATH wrong: ' + h2.constants.HTTP2_HEADER_PATH);\n\
         }\n\
         if (typeof h2.createServer !== 'function') throw new Error('createServer missing');\n\
         if (typeof h2.connect !== 'function') throw new Error('connect missing');\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn http2_server_throws_client_returns_session() {
    assert_passes(
        "const h2 = require('node:http2');\n\
         let threw = false;\n\
         try { h2.createServer(); } catch { threw = true; }\n\
         if (!threw) throw new Error('createServer should throw');\n\
         threw = false;\n\
         try { h2.createSecureServer(); } catch { threw = true; }\n\
         if (!threw) throw new Error('createSecureServer should throw');\n\
         const session = h2.connect('https://example.com');\n\
         if (!session || typeof session.request !== 'function') {\n\
           throw new Error('connect should return a session with request()');\n\
         }\n\
         session.destroy();\n",
    )
    .await;
}

// ---------- readline + readline/promises ----------

#[tokio::test(flavor = "current_thread")]
async fn readline_emits_line_events_for_lf_and_crlf() {
    assert_passes(
        "(async () => {\n\
           const EventEmitter = require('node:events');\n\
           const rl = require('node:readline');\n\
           const input = new EventEmitter();\n\
           const iface = rl.createInterface({ input });\n\
           const lines = [];\n\
           iface.on('line', (l) => lines.push(l));\n\
           const closed = new Promise((r) => iface.on('close', r));\n\
           // mix of \\n, \\r\\n and split chunks\n\
           input.emit('data', 'alpha\\nbet');\n\
           input.emit('data', 'a\\r\\ngamma');\n\
           input.emit('data', '\\n');\n\
           input.emit('data', 'final-no-newline');\n\
           input.emit('end');\n\
           await closed;\n\
           const want = ['alpha', 'beta', 'gamma', 'final-no-newline'];\n\
           if (JSON.stringify(lines) !== JSON.stringify(want)) {\n\
             throw new Error('lines: ' + JSON.stringify(lines));\n\
           }\n\
         })().catch((err) => { throw err; });\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn readline_async_iterator_yields_lines() {
    assert_passes(
        "(async () => {\n\
           const EventEmitter = require('node:events');\n\
           const rl = require('node:readline');\n\
           const input = new EventEmitter();\n\
           const iface = rl.createInterface({ input });\n\
           // Feed lines on next ticks so the iterator has to wait.\n\
           setTimeout(() => input.emit('data', 'one\\ntwo\\n'), 0);\n\
           setTimeout(() => { input.emit('data', 'three\\n'); input.emit('end'); }, 1);\n\
           const out = [];\n\
           for await (const line of iface) out.push(line);\n\
           if (JSON.stringify(out) !== JSON.stringify(['one','two','three'])) {\n\
             throw new Error('iterator yielded: ' + JSON.stringify(out));\n\
           }\n\
         })().catch((err) => { throw err; });\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn readline_question_resolves_on_next_line() {
    assert_passes(
        "(async () => {\n\
           const EventEmitter = require('node:events');\n\
           const rl = require('node:readline');\n\
           const input = new EventEmitter();\n\
           const writes = [];\n\
           const output = { write(d) { writes.push(d); } };\n\
           const iface = rl.createInterface({ input, output });\n\
           const answer = await new Promise((resolve) => {\n\
             iface.question('name? ', resolve);\n\
             setTimeout(() => input.emit('data', 'alice\\n'), 0);\n\
           });\n\
           if (answer !== 'alice') throw new Error('answer: ' + answer);\n\
           if (writes.join('') !== 'name? ') {\n\
             throw new Error('prompt not written: ' + writes.join(''));\n\
           }\n\
         })().catch((err) => { throw err; });\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn readline_close_is_idempotent_and_drains_pending() {
    assert_passes(
        "(async () => {\n\
           const EventEmitter = require('node:events');\n\
           const rl = require('node:readline');\n\
           const input = new EventEmitter();\n\
           const iface = rl.createInterface({ input });\n\
           let closeCount = 0;\n\
           iface.on('close', () => closeCount++);\n\
           // Pending question() should resolve with '' on close.\n\
           const p = new Promise((resolve) => iface.question('?', resolve));\n\
           iface.close();\n\
           iface.close();\n\
           const v = await p;\n\
           if (v !== '') throw new Error('closed question must yield empty: ' + v);\n\
           if (closeCount !== 1) throw new Error('close fired ' + closeCount + 'x');\n\
         })().catch((err) => { throw err; });\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn readline_decodes_split_utf8_chunks() {
    assert_passes(
        "(async () => {\n\
           const EventEmitter = require('node:events');\n\
           const rl = require('node:readline');\n\
           const input = new EventEmitter();\n\
           const iface = rl.createInterface({ input });\n\
           const lines = [];\n\
           iface.on('line', (l) => lines.push(l));\n\
           const closed = new Promise((r) => iface.on('close', r));\n\
           // 'mañana' = 6d 61 c3 b1 61 6e 61 ; split mid c3/b1 boundary.\n\
           input.emit('data', new Uint8Array([0x6d, 0x61, 0xc3]));\n\
           input.emit('data', new Uint8Array([0xb1, 0x61, 0x6e, 0x61, 0x0a]));\n\
           input.emit('end');\n\
           await closed;\n\
           if (lines.length !== 1 || lines[0] !== 'mañana') {\n\
             throw new Error('utf8 reassembly: ' + JSON.stringify(lines));\n\
           }\n\
         })().catch((err) => { throw err; });\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn readline_promises_question_resolves() {
    assert_passes(
        "(async () => {\n\
           const EventEmitter = require('node:events');\n\
           const rl = require('node:readline/promises');\n\
           const input = new EventEmitter();\n\
           const iface = rl.createInterface({ input, output: { write(){} } });\n\
           setTimeout(() => input.emit('data', 'yes\\n'), 0);\n\
           const ans = await iface.question('confirm? ');\n\
           if (ans !== 'yes') throw new Error('answer: ' + ans);\n\
           iface.close();\n\
         })().catch((err) => { throw err; });\n",
    )
    .await;
}

// ---------- punycode (sanity, was added earlier) ----------

#[tokio::test(flavor = "current_thread")]
async fn punycode_round_trips_idn() {
    assert_passes(
        "const p = require('node:punycode');\n\
         const enc = p.toASCII('mañana.com');\n\
         if (enc !== 'xn--maana-pta.com') throw new Error('toASCII: ' + enc);\n\
         const dec = p.toUnicode('xn--maana-pta.com');\n\
         if (dec !== 'mañana.com') throw new Error('toUnicode: ' + dec);\n",
    )
    .await;
}
