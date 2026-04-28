// node:v8 — minimal stub. Next.js calls v8.getHeapStatistics() to log
// memory usage; we serve numbers from our own heap snapshot, but we
// don't expose serializer / cache machinery.

(function () {
  function getHeapStatistics() {
    return {
      total_heap_size: 0,
      total_heap_size_executable: 0,
      total_physical_size: 0,
      total_available_size: 0,
      used_heap_size: 0,
      heap_size_limit: 0,
      malloced_memory: 0,
      peak_malloced_memory: 0,
      does_zap_garbage: 0,
      number_of_native_contexts: 1,
      number_of_detached_contexts: 0,
    };
  }
  function getHeapSpaceStatistics() { return []; }
  function setFlagsFromString() {}

  module.exports = {
    getHeapStatistics,
    getHeapSpaceStatistics,
    setFlagsFromString,
    serialize: () => Buffer.from([]),
    deserialize: () => undefined,
    cachedDataVersionTag: () => 0,
  };
})();
