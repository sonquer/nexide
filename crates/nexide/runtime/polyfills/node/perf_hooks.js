// node:perf_hooks - minimal `performance` surface backed by Date.now.

(function () {
  const start = Date.now();
  const performance = {
    now() {
      return Date.now() - start;
    },
    timeOrigin: start,
    mark() {},
    measure() {},
    clearMarks() {},
    clearMeasures() {},
    getEntries() { return []; },
    getEntriesByName() { return []; },
    getEntriesByType() { return []; },
  };

  class PerformanceObserver {
    constructor() {}
    observe() {}
    disconnect() {}
  }
  PerformanceObserver.supportedEntryTypes = [];

  module.exports = {
    performance,
    PerformanceObserver,
    monitorEventLoopDelay() {
      return {
        enable() {}, disable() {}, reset() {},
        min: 0, max: 0, mean: 0, stddev: 0, percentile() { return 0; },
      };
    },
  };
})();
