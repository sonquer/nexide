// node:vm - minimal stub. The full vm machinery (separate isolate
// realms) is not in scope for nexide. We expose runInThisContext as
// a thin eval wrapper so libraries that probe `vm` for "is this Node?"
// don't crash on import.

(function () {
  function runInThisContext(code) {
    if (typeof code !== "string") {
      throw new TypeError("runInThisContext requires a string");
    }
    return (0, eval)(code);
  }

  function runInNewContext(code, sandbox) {
    if (typeof code !== "string") {
      throw new TypeError("runInNewContext requires a string");
    }
    if (sandbox == null) {
      return (0, eval)(code);
    }
    const fn = new Function(
      "sandbox",
      "var self = sandbox; var globalThis = sandbox; var global = sandbox; with (sandbox) { " + code + " }"
    );
    return fn(sandbox);
  }

  function compileFunction(code, params) {
    if (typeof code !== "string") {
      throw new TypeError("compileFunction requires a string");
    }
    const list = Array.isArray(params) ? params : [];
    const safe = /^[A-Za-z_$][A-Za-z0-9_$]*$/;
    for (const p of list) {
      if (typeof p !== "string" || !safe.test(p)) {
        const err = new TypeError(
          "compileFunction params must be valid JavaScript identifiers"
        );
        err.code = "ERR_INVALID_ARG_VALUE";
        throw err;
      }
    }
    return new Function(list.join(","), code);
  }

  module.exports = {
    runInThisContext,
    runInNewContext,
    runInContext: runInNewContext,
    compileFunction,
    createContext: (sandbox) => sandbox || {},
    isContext: () => false,
    Script: class Script {
      constructor(code) { this._code = code; }
      runInThisContext() { return runInThisContext(this._code); }
      runInNewContext(ctx) { return runInNewContext(this._code, ctx); }
      runInContext(ctx) { return runInNewContext(this._code, ctx); }
    },
  };
})();
