// Shim for the small Nexide.core surface beyond ops/print/queueMicrotask.
//
// Provides `AsyncVariable`, `getAsyncContext`, `setAsyncContext`,
// backed by V8's Continuation-Preserved Embedder Data. The native ops
// `Nexide.core.getAsyncContext` and `Nexide.core.setAsyncContext` are
// installed from Rust and read/write CPED, which V8 automatically
// propagates across `await`, `.then()`, `queueMicrotask()`, and timer
// resumptions — matching Node.js v22+ AsyncLocalStorage semantics.

((globalThis) => {
  "use strict";
  const core = globalThis.Nexide && globalThis.Nexide.core;
  if (!core) return;
  if (core.AsyncVariable) return;

  const nativeGet = core.getAsyncContext;
  const nativeSet = core.setAsyncContext;

  function snapshot() {
    const current = nativeGet();
    if (current && typeof current === "object" && current.__nexideCtx) {
      return current;
    }
    return Object.create(null);
  }

  function publish(snap) {
    Object.defineProperty(snap, "__nexideCtx", {
      value: true,
      enumerable: false,
      configurable: false,
      writable: false,
    });
    nativeSet(snap);
  }

  let nextId = 1;

  class AsyncVariable {
    constructor() {
      this._id = nextId++;
    }
    enter(value) {
      const previous = snapshot();
      const next = Object.assign(Object.create(null), previous);
      next[this._id] = value;
      publish(next);
      return previous;
    }
    get() {
      const cur = snapshot();
      return cur[this._id];
    }
  }

  function getAsyncContext() {
    return snapshot();
  }

  function setAsyncContext(snap) {
    if (snap && typeof snap === "object") {
      publish(snap);
    } else {
      publish(Object.create(null));
    }
  }

  core.AsyncVariable = AsyncVariable;
  core.getAsyncContext = getAsyncContext;
  core.setAsyncContext = setAsyncContext;
})(globalThis);
