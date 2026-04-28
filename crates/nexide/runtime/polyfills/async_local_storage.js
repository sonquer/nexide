// AsyncLocalStorage polyfill for the nexide runtime.
//
// Implements the Node.js `node:async_hooks` surface that App Router
// Next.js relies on (cookies/headers/draftMode propagation). Backed
// by V8's continuation-preserved-embedder-data (CPED) exposed
// through `Nexide.core.AsyncVariable`, which automatically
// propagates context across all async boundaries: `await`,
// `Promise.then`, `queueMicrotask`, `setTimeout`, and other
// microtasks. This matches modern Node.js (v22+) semantics, so no
// promise hooks or scheduler shims are required.
//
// The polyfill is idempotent: re-loading it is a no-op so
// installation hooks can run more than once without duplicating
// registrations.

((globalThis) => {
  "use strict";

  if (globalThis.AsyncLocalStorage && globalThis.AsyncLocalStorage.__nexide) {
    return;
  }

  const { AsyncVariable, getAsyncContext, setAsyncContext } = Nexide.core;

  class AsyncLocalStorage {
    static __nexide = true;

    #variable = new AsyncVariable();
    #disabled = false;
    #defaultValue;

    constructor(options = {}) {
      this.#defaultValue = options && options.defaultValue;
    }

    run(store, callback, ...args) {
      if (this.#disabled) {
        return Reflect.apply(callback, null, args);
      }
      const previous = this.#variable.enter(store);
      try {
        return Reflect.apply(callback, null, args);
      } finally {
        setAsyncContext(previous);
      }
    }

    getStore() {
      if (this.#disabled) {
        return this.#defaultValue;
      }
      const value = this.#variable.get();
      return value === undefined ? this.#defaultValue : value;
    }

    enterWith(store) {
      if (this.#disabled) {
        return;
      }
      this.#variable.enter(store);
    }

    exit(callback, ...args) {
      const wasDisabled = this.#disabled;
      this.#disabled = true;
      try {
        return Reflect.apply(callback, null, args);
      } finally {
        this.#disabled = wasDisabled;
      }
    }

    disable() {
      this.#disabled = true;
    }

    static bind(fn) {
      return AsyncResource.bind(fn);
    }

    static snapshot() {
      const resource = new AsyncResource("AsyncLocalStorage.snapshot");
      return function runInSnapshot(cb, ...args) {
        return resource.runInAsyncScope(cb, null, ...args);
      };
    }
  }

  class AsyncResource {
    #snapshot;

    constructor(_type) {
      this.#snapshot = getAsyncContext();
    }

    runInAsyncScope(fn, thisArg, ...args) {
      const previous = getAsyncContext();
      setAsyncContext(this.#snapshot);
      try {
        return Reflect.apply(fn, thisArg, args);
      } finally {
        setAsyncContext(previous);
      }
    }

    bind(fn, thisArg) {
      const resource = this;
      const bound = function boundInResource(...args) {
        return resource.runInAsyncScope(fn, thisArg ?? this, ...args);
      };
      Object.defineProperty(bound, "length", {
        configurable: true,
        enumerable: false,
        value: typeof fn === "function" ? fn.length : 0,
        writable: false,
      });
      return bound;
    }

    static bind(fn, type) {
      return new AsyncResource(type || fn.name || "bound-anonymous-fn").bind(fn);
    }
  }

  globalThis.AsyncLocalStorage = AsyncLocalStorage;
  globalThis.AsyncResource = AsyncResource;
  globalThis.__nexideHooksInstalled =
    (globalThis.__nexideHooksInstalled || 0) + 1;
})(globalThis);
