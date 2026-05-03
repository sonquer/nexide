// Pump error-path fixture.
//
// Installs `globalThis.__nexide.__dispatch` as a handler whose
// behaviour is selected by the request URI:
//
//   /sync-throw   → throws synchronously
//   /async-reject → returns a Promise that rejects on the next tick
//   /ok           → completes a normal 200 response
//
// The integration test boots an engine on this fixture, starts the
// pump, and enqueues requests against the URIs above to assert that
// `op_nexide_finish_error` is wired all the way through.

globalThis.__nexide.__dispatch = function (idx, gen) {
  const meta = globalThis.__nexide.getMeta(idx, gen);
  const uri = meta[1];
  if (uri === "/sync-throw") {
    throw new Error("sync-boom");
  }
  if (uri === "/async-reject") {
    return Promise.resolve().then(() => {
      throw new Error("async-boom");
    });
  }
  globalThis.__nexide.sendHead(idx, gen, 200, [["x-uri", uri]]);
  globalThis.__nexide.sendEnd(idx, gen);
  return undefined;
};
