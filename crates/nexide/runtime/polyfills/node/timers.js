// node:timers - re-export the host scheduling primitives so libraries can
// import them explicitly. Promise-flavoured helpers live in
// node:timers/promises.

(function () {
  module.exports = {
    setTimeout: globalThis.setTimeout,
    clearTimeout: globalThis.clearTimeout,
    setInterval: globalThis.setInterval,
    clearInterval: globalThis.clearInterval,
    setImmediate: globalThis.setImmediate,
    clearImmediate: globalThis.clearImmediate,
    queueMicrotask: globalThis.queueMicrotask,
  };
})();
