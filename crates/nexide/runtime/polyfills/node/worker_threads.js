"use strict";

/**
 * Polyfill for Node.js `node:worker_threads`.
 *
 * The nexide runtime executes a single V8 isolate per HTTP worker
 * (`SO_REUSEPORT` fan-out at the kernel level), so spawning a
 * JS-level `Worker` would require booting a second isolate on its
 * own OS thread - not supported in the standalone Next.js deployment
 * shape this runtime targets.
 *
 * The module-level constants are kept fully functional:
 *   * `isMainThread` is always `true`,
 *   * `parentPort` is `null`,
 *   * `workerData` is `undefined`,
 *   * `threadId` is `0`.
 *
 * That matches what user code observes when running on the main
 * thread of Node.js, so the common pattern
 * `if (isMainThread) { /* primary */ } else { /* worker */ }`
 * always takes the primary branch under nexide.
 *
 * Constructing `new Worker(...)` throws an `Error` with
 * `code = "ERR_NOT_AVAILABLE"` so applications that genuinely
 * require thread-level parallelism see a clear failure with a
 * concrete migration hint (the suggested alternative is
 * `child_process.spawn` for CPU-bound work that does not need
 * shared memory).
 */

const EventEmitter = require("node:events");

class MessageChannel {
  constructor() {
    const err = new Error(
      "MessageChannel is not available in nexide; use child_process for parallel work",
    );
    err.code = "ERR_NOT_AVAILABLE";
    throw err;
  }
}

class Worker extends EventEmitter {
  constructor() {
    super();
    const err = new Error(
      "worker_threads.Worker is not available in nexide; use child_process.spawn or fan out across HTTP workers",
    );
    err.code = "ERR_NOT_AVAILABLE";
    throw err;
  }
}

const moduleExports = {
  isMainThread: true,
  parentPort: null,
  workerData: undefined,
  threadId: 0,
  Worker,
  MessageChannel,
  receiveMessageOnPort: () => undefined,
  setEnvironmentData: () => undefined,
  getEnvironmentData: () => undefined,
  markAsUntransferable: () => undefined,
  moveMessagePortToContext: () => {
    const err = new Error("moveMessagePortToContext requires a worker isolate");
    err.code = "ERR_NOT_AVAILABLE";
    throw err;
  },
};
moduleExports.default = moduleExports;

module.exports = moduleExports;
