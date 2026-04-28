// node:async_hooks - minimal AsyncLocalStorage shim.
//
// Next.js relies on `AsyncLocalStorage` for request-scoped state.
// Real Node.js implements it on top of the AsyncWrap machinery.
// nexide's runtime is single-isolate-per-request: each request runs
// to completion on the same microtask scheduler before the next one
// is dispatched, so a process-global slot is sufficient - the JS
// callback runs synchronously inside `run`/`enterWith`, and stores
// nest as a stack.

(function () {
  class AsyncLocalStorage {
    constructor() {
      this._stack = [];
    }
    getStore() {
      const len = this._stack.length;
      return len === 0 ? undefined : this._stack[len - 1];
    }
    run(store, callback, ...args) {
      this._stack.push(store);
      try {
        return callback(...args);
      } finally {
        this._stack.pop();
      }
    }
    enterWith(store) {
      this._stack.push(store);
    }
    exit(callback, ...args) {
      const saved = this._stack.slice();
      this._stack.length = 0;
      try {
        return callback(...args);
      } finally {
        this._stack = saved;
      }
    }
    disable() {
      this._stack.length = 0;
    }
    static bind(fn) {
      return fn;
    }
    static snapshot() {
      return function runInSnapshot(cb, ...args) {
        return cb(...args);
      };
    }
  }

  function executionAsyncId() {
    return 0;
  }
  function triggerAsyncId() {
    return 0;
  }

  function createHook() {
    return {
      enable() { return this; },
      disable() { return this; },
    };
  }

  const ALS = globalThis.AsyncLocalStorage || AsyncLocalStorage;
  const AR = globalThis.AsyncResource || class AsyncResource {
    constructor(type) { this.type = type; }
    runInAsyncScope(fn, thisArg, ...args) {
      return fn.apply(thisArg, args);
    }
    bind(fn) { return fn.bind(null); }
    static bind(fn) { return fn.bind(null); }
  };

  module.exports = {
    AsyncLocalStorage: ALS,
    AsyncResource: AR,
    executionAsyncId,
    triggerAsyncId,
    createHook,
  };
})();
