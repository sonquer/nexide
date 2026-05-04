// node:inspector - lightweight Inspector emulation.
//
// nexide does not embed the V8 Inspector C++ protocol or speak the
// Chrome DevTools wire format, so this module cannot host a real
// debugger or live CPU profiler. What it can do is route a small
// set of high-frequency `Session.post` calls that APM agents
// (Datadog, Sentry, Elastic) issue at startup to inspect runtime
// state, so those agents observe sensible answers instead of empty
// objects:
//
//   * `Runtime.evaluate` - executes the supplied expression in the
//     current realm via `vm.runInThisContext` and returns a result
//     descriptor matching the inspector wire shape.
//   * `Runtime.getHeapUsage` / `Runtime.getHeapStatistics` -
//     forwards to `process.memoryUsage()` and returns it in the
//     shape the inspector emits.
//   * `HeapProfiler.collectGarbage` - calls `globalThis.gc()` if
//     available (only when `--expose-gc` is set on the CLI).
//   * `Profiler.enable`/`disable` - acknowledged but unsupported.
//
// All other methods reject with an `code: -32601` ("Method not
// found") error, mirroring the Inspector protocol.

(function () {
  "use strict";
  const noop = function () {};
  const vm = require("node:vm");

  function methodNotFound(method) {
    const err = new Error(`'${method}' wasn't found`);
    err.code = -32601;
    return err;
  }

  function handle(method, params) {
    switch (method) {
      case "Runtime.evaluate": {
        const expression = params && params.expression;
        if (typeof expression !== "string") {
          throw new TypeError("Runtime.evaluate requires `params.expression`");
        }
        try {
          const value = vm.runInThisContext(expression);
          return {
            result: {
              type: typeof value,
              value: value === undefined ? null : value,
              description: String(value),
            },
          };
        } catch (e) {
          return {
            exceptionDetails: {
              text: String(e && e.message),
              exception: { type: "object", description: String(e) },
            },
          };
        }
      }
      case "Runtime.getHeapUsage":
      case "Runtime.getHeapStatistics": {
        const m = process.memoryUsage();
        return {
          usedSize: m.heapUsed,
          totalSize: m.heapTotal,
          ...m,
        };
      }
      case "HeapProfiler.collectGarbage": {
        if (typeof globalThis.gc === "function") globalThis.gc();
        return {};
      }
      case "Profiler.enable":
      case "Profiler.disable":
      case "Debugger.enable":
      case "Debugger.disable":
      case "Runtime.enable":
      case "Runtime.disable":
        return {};
      default:
        throw methodNotFound(method);
    }
  }

  class Session {
    constructor() { this._connected = false; }
    connect() { this._connected = true; }
    connectToMainThread() { this._connected = true; }
    disconnect() { this._connected = false; }
    post(method, params, cb) {
      if (typeof params === "function") { cb = params; params = undefined; }
      if (!this._connected) {
        const err = new Error("inspector session is not connected");
        if (typeof cb === "function") queueMicrotask(() => cb(err));
        return;
      }
      try {
        const result = handle(String(method), params || {});
        if (typeof cb === "function") queueMicrotask(() => cb(null, result));
      } catch (err) {
        if (typeof cb === "function") queueMicrotask(() => cb(err));
      }
    }
    on() { return this; }
    once() { return this; }
    off() { return this; }
    addListener() { return this; }
    removeListener() { return this; }
    emit() { return false; }
  }

  module.exports = {
    Session,
    open: noop,
    close: noop,
    url: () => undefined,
    waitForDebugger: noop,
    console: globalThis.console,
  };
})();
