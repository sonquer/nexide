// node:vm — backed by real `v8::Context` instances.
//
// Each call to `createContext` allocates a fresh V8 context on the
// running isolate. The returned sandbox **is** the new context's
// `globalThis`; mutating the sandbox from the host realm (e.g.
// `ctx.Response = ...`) writes the property onto the new context's
// global object, and code evaluated via `runInContext` sees those
// mutations as bare globals.
//
// Behavioural notes (vs Node `vm`):
//   * One isolate, many contexts. `instanceof Array` across the host
//     realm and a sandbox realm yields `false` (real Realm boundary).
//   * Microtask queues are shared with the host isolate, so Promises
//     created inside the sandbox are drained by the same event loop.
//   * `vm` is **not** a security boundary in nexide — sandbox code
//     runs on the same OS thread as the host. Treat the sandbox as a
//     correctness tool, not as untrusted-code isolation.

(function () {
  const ops = (typeof Nexide !== "undefined" && Nexide && Nexide.core && Nexide.core.ops) || null;

  function runInThisContext(code, options) {
    if (typeof code !== "string") {
      throw new TypeError("runInThisContext requires a string");
    }
    if (options && typeof options === "object" && typeof options.filename === "string") {
      return (0, eval)(code + "\n//# sourceURL=" + options.filename);
    }
    return (0, eval)(code);
  }

  function createContext(seed) {
    if (!ops || typeof ops.op_vm_create_context !== "function") {
      return seed && typeof seed === "object" ? seed : {};
    }
    return ops.op_vm_create_context(seed && typeof seed === "object" ? seed : null);
  }

  function isContext(sandbox) {
    if (!ops || typeof ops.op_vm_is_context !== "function") return false;
    return !!ops.op_vm_is_context(sandbox);
  }

  function runInContext(code, sandbox, options) {
    if (typeof code !== "string") {
      throw new TypeError("runInContext requires a string");
    }
    if (!sandbox || typeof sandbox !== "object") {
      throw new TypeError("runInContext requires a context object");
    }
    if (!ops || typeof ops.op_vm_run_in_context !== "function") {
      return (0, eval)(code);
    }
    const filename =
      options && typeof options === "object" && typeof options.filename === "string"
        ? options.filename
        : "[vm:runInContext]";
    return ops.op_vm_run_in_context(sandbox, code, filename);
  }

  function runInNewContext(code, sandbox, options) {
    const userObj = sandbox && typeof sandbox === "object" ? sandbox : null;
    const ctx = createContext(userObj);
    if (userObj && ctx !== userObj) {
      try {
        const keys = Object.getOwnPropertyNames(userObj);
        for (const k of keys) {
          try { ctx[k] = userObj[k]; } catch { /* read-only host prop */ }
        }
      } catch { /* ignore */ }
    }
    const result = runInContext(code, ctx, options);
    if (userObj && ctx !== userObj) {
      try {
        const keys = Object.getOwnPropertyNames(ctx);
        for (const k of keys) {
          try { userObj[k] = ctx[k]; } catch { /* ignore */ }
        }
      } catch { /* ignore */ }
    }
    return result;
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

  class Script {
    constructor(code, options) {
      if (typeof code !== "string") {
        throw new TypeError("Script requires a string");
      }
      this._code = code;
      this._filename =
        options && typeof options === "object" && typeof options.filename === "string"
          ? options.filename
          : "[vm:Script]";
    }
    runInThisContext() {
      return runInThisContext(this._code, { filename: this._filename });
    }
    runInNewContext(ctx) {
      return runInNewContext(this._code, ctx, { filename: this._filename });
    }
    runInContext(ctx) {
      return runInContext(this._code, ctx, { filename: this._filename });
    }
  }

  module.exports = {
    runInThisContext,
    runInNewContext,
    runInContext,
    compileFunction,
    createContext,
    isContext,
    Script,
  };
})();
