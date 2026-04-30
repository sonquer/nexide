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

  function runInNewContext(code, sandbox, options) {
    if (typeof code !== "string") {
      throw new TypeError("runInNewContext requires a string");
    }
    if (sandbox == null) {
      return (0, eval)(code);
    }
    ensureContextEventTarget(sandbox);

    var keys = Object.keys(sandbox);
    for (var i = 0; i < keys.length; i++) {
      var k = keys[i];
      if (k === "globalThis" || k === "global" || k === "self") continue;
      try { globalThis[k] = sandbox[k]; } catch (_) {}
    }
    var src = "(function(){\n" + code + "\n})()";
    if (options && typeof options === "object" && typeof options.filename === "string") {
      src += "\n//# sourceURL=" + options.filename;
    }
    return (0, eval)(src);
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

  function ensureContextEventTarget(sandbox) {
    if (!sandbox || typeof sandbox !== "object") return sandbox;
    if (typeof sandbox.addEventListener === "function") return sandbox;
    var ET = globalThis.EventTarget;
    var et = ET ? new ET() : null;
    if (!et) {
      et = (function () {
        var listeners = Object.create(null);
        return {
          addEventListener: function (type, fn) {
            (listeners[type] || (listeners[type] = [])).push(fn);
          },
          removeEventListener: function (type, fn) {
            var arr = listeners[type]; if (!arr) return;
            var i = arr.indexOf(fn); if (i >= 0) arr.splice(i, 1);
          },
          dispatchEvent: function (event) {
            var arr = listeners[event && event.type]; if (!arr) return true;
            for (var i = 0; i < arr.length; i++) {
              try { arr[i].call(sandbox, event); } catch (_) {}
            }
            return !(event && event.defaultPrevented);
          },
        };
      })();
    }
    sandbox.addEventListener = function () { return et.addEventListener.apply(et, arguments); };
    sandbox.removeEventListener = function () { return et.removeEventListener.apply(et, arguments); };
    sandbox.dispatchEvent = function () { return et.dispatchEvent.apply(et, arguments); };
    return sandbox;
  }

  module.exports = {
    runInThisContext,
    runInNewContext,
    runInContext: runInNewContext,
    compileFunction,
    createContext: (sandbox) => ensureContextEventTarget(sandbox || {}),
    isContext: () => false,
    Script: class Script {
      constructor(code) { this._code = code; }
      runInThisContext() { return runInThisContext(this._code); }
      runInNewContext(ctx) { return runInNewContext(this._code, ctx); }
      runInContext(ctx) { return runInNewContext(this._code, ctx); }
    },
  };
})();
