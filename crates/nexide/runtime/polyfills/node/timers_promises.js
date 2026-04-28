// node:timers/promises - promise-flavoured timer helpers.

(function () {
  function setTimeoutP(ms, value, options) {
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => resolve(value), ms);
      if (options && options.signal) {
        if (options.signal.aborted) {
          clearTimeout(timer);
          reject(options.signal.reason ?? new Error("AbortError"));
          return;
        }
        options.signal.addEventListener("abort", () => {
          clearTimeout(timer);
          reject(options.signal.reason ?? new Error("AbortError"));
        });
      }
    });
  }
  function setImmediateP(value, options) {
    return setTimeoutP(0, value, options);
  }
  async function* setIntervalP(ms, value, _options) {
    while (true) {
      await setTimeoutP(ms);
      yield value;
    }
  }
  module.exports = {
    setTimeout: setTimeoutP,
    setImmediate: setImmediateP,
    setInterval: setIntervalP,
    scheduler: { wait: setTimeoutP, yield: () => setTimeoutP(0) },
  };
})();
