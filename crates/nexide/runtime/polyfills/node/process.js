// node:process - re-export the global process object as a CJS module.
// Some libraries (commander, ESM-converted code) explicitly require
// 'node:process' instead of relying on the global.

(function () {
  module.exports = globalThis.process;
})();
