// node:inspector - minimal stub. Next.js consults this for debugger
// hooks, which are no-ops in our runtime.

(function () {
  const noop = function () {};
  class Session {
    connect() {}
    disconnect() {}
    post(_method, _params, cb) { if (typeof cb === 'function') cb(null, {}); }
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
