// Late-stage polyfill: expose Node-flavoured globals that depend on the
// CJS loader being installed. Runs after cjs_loader so `require` is
// available, and re-exports URL classes from `node:url` plus
// AsyncLocalStorage from `node:async_hooks` onto `globalThis`.
//
// Note: AsyncLocalStorage context propagation is handled natively by
// V8's continuation-preserved-embedder-data (via `Nexide.core.AsyncVariable`),
// which automatically threads through `await`, `queueMicrotask`,
// `setTimeout`, and other microtask boundaries. No scheduler shim is
// required.

(function () {
  if (typeof require === "function") {
    try {
      const url = require("node:url");
      if (typeof globalThis.URL === "undefined" && url.URL) globalThis.URL = url.URL;
      if (typeof globalThis.URLSearchParams === "undefined" && url.URLSearchParams) {
        globalThis.URLSearchParams = url.URLSearchParams;
      }
    } catch { }
    try {
      const ah = require("node:async_hooks");
      if (typeof globalThis.AsyncLocalStorage === "undefined" && ah.AsyncLocalStorage) {
        globalThis.AsyncLocalStorage = ah.AsyncLocalStorage;
      }
    } catch { }
    try {
      const c = require("node:crypto");
      if (typeof globalThis.crypto === "undefined" && c.webcrypto) {
        Object.defineProperty(globalThis, "crypto", {
          value: c.webcrypto, writable: true, configurable: true, enumerable: false,
        });
      }
      const cryptoClasses = ["Crypto", "CryptoKey", "SubtleCrypto"];
      for (let i = 0; i < cryptoClasses.length; i++) {
        const name = cryptoClasses[i];
        if (typeof globalThis[name] === "undefined" && c[name]) {
          Object.defineProperty(globalThis, name, {
            value: c[name], writable: true, configurable: true, enumerable: false,
          });
        }
      }
    } catch { }
  }
})();
