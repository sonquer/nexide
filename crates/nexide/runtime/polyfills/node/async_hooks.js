// node:async_hooks
//
// nexide's `globalThis.AsyncLocalStorage` is installed at boot from
// `runtime/polyfills/async_local_storage.js` and is backed by V8's
// continuation-preserved-embedder-data (CPED). That global is the
// single source of truth - context propagates correctly across
// `await`, `Promise.then`, `queueMicrotask`, and `setTimeout`.
//
// This module re-exports the global rather than shipping a
// stack-based fallback. A stack-based ALS would silently lose
// context across `await` boundaries, so if the global is somehow
// missing we fail fast at module load instead of handing back a
// broken implementation.
//
// `AsyncResource` mirrors the same idea - it uses
// `AsyncLocalStorage.snapshot()` to capture the current context at
// construction time and replay it inside `runInAsyncScope`.

(function () {
  "use strict";

  if (typeof globalThis.AsyncLocalStorage !== "function") {
    throw new Error(
      "nexide: globalThis.AsyncLocalStorage is unavailable. The CPED-backed "
        + "polyfill must be loaded before require('node:async_hooks'). "
        + "Check the bootstrap order in crates/nexide/src/engine/v8_engine/bootstrap.rs.",
    );
  }
  const ALS = globalThis.AsyncLocalStorage;

  class AsyncResource {
    constructor(type, _opts) {
      this.type = String(type || "AsyncResource");
      this._snapshot = ALS.snapshot();
    }
    runInAsyncScope(fn, thisArg, ...args) {
      return this._snapshot.call(thisArg, () => fn.apply(thisArg, args));
    }
    bind(fn, thisArg) {
      const snap = this._snapshot;
      const bound = function bound(...args) {
        return snap.call(thisArg, () => fn.apply(thisArg, args));
      };
      bound.asyncResource = this;
      return bound;
    }
    static bind(fn, type, thisArg) {
      const r = new AsyncResource(type || fn.name || "bound-anonymous-fn");
      return r.bind(fn, thisArg);
    }
    emitDestroy() { return this; }
    asyncId() { return 0; }
    triggerAsyncId() { return 0; }
  }

  const AR = (typeof globalThis.AsyncResource === "function"
    && globalThis.AsyncResource !== AsyncResource)
    ? globalThis.AsyncResource
    : AsyncResource;

  module.exports = {
    AsyncLocalStorage: ALS,
    AsyncResource: AR,
    executionAsyncId: () => 0,
    triggerAsyncId: () => 0,
    executionAsyncResource: () => ({}),
    createHook: () => ({ enable() { return this; }, disable() { return this; } }),
  };
})();
