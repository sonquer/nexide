// Test handler: reads request meta, body, and writes a response.
// Exposed as `globalThis.__nexideRunHandler` so the integration test
// can trigger it after planting a request slot. The driver passes the
// `(idx, gen)` request id pair issued by the engine.
//
// The nexide isolate boots without Web globals (`TextEncoder`, …) by
// default, so this fixture stays on raw `Uint8Array` views to keep
// the test self-contained.

globalThis.__nexideRunHandler = function (idx, gen) {
  const meta = globalThis.__nexide.getMeta(idx, gen);

  const buf = new Uint8Array(64);
  const n = globalThis.__nexide.readBody(idx, gen, buf);
  const bodyView = buf.subarray(0, n);

  globalThis.__nexide.sendHead(idx, gen, 200, [
    ["content-type", "text/plain"],
    ["x-method", meta.method],
    ["x-uri", meta.uri],
  ]);

  const prefix = new Uint8Array([0x70, 0x6f, 0x6e, 0x67, 0x3a]);
  const out = new Uint8Array(prefix.length + bodyView.length);
  out.set(prefix, 0);
  out.set(bodyView, prefix.length);
  globalThis.__nexide.sendChunk(idx, gen, out);
  globalThis.__nexide.sendEnd(idx, gen);
};
