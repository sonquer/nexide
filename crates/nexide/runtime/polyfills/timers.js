"use strict";

/**
 * Timer polyfill (`setTimeout` / `setInterval` / `setImmediate`).
 *
 * `setTimeout` and `setInterval` are honest timers: they call
 * `op_timer_sleep(ms)` on the host, which is implemented with
 * `tokio::time::sleep` and resumes the JS callback through the
 * async-completion pump. Delays are clamped to non-negative
 * integers, mirroring Node's coercion rules.
 *
 * `setImmediate` is a true macrotask: it resolves on the next
 * event-loop tick after the current microtask queue drains. It
 * relies on `op_void_async_deferred`, which Next.js' streaming SSR
 * needs to flush chunks between renders.
 *
 * Cancelled timers are tracked by id in a single `Set`. The
 * implementation is idempotent so the file can safely be evaluated
 * once per isolate.
 */

((globalThis) => {
  if (globalThis.__nexideTimersInstalled) return;

  const ops = (Nexide && Nexide.core && Nexide.core.ops) || {};
  const opTimerSleep = ops.op_timer_sleep;
  const opVoidDeferred = ops.op_void_async_deferred;

  if (typeof opTimerSleep !== "function") {
    throw new Error("nexide: op_timer_sleep is not registered");
  }
  if (typeof opVoidDeferred !== "function") {
    throw new Error("nexide: op_void_async_deferred is not registered");
  }

  let nextId = 1;
  const cancelled = new Set();

  function nextTimerId() {
    const id = nextId++;
    if (nextId > 0x7fffffff) nextId = 1;
    return id;
  }

  function coerceDelay(ms) {
    const n = Number(ms);
    if (!Number.isFinite(n) || n < 0) return 0;
    return Math.floor(n);
  }

  function runOnce(id, cb, args) {
    if (cancelled.delete(id)) return;
    cb(...args);
  }

  globalThis.setTimeout = function setTimeout(cb, ms, ...args) {
    if (typeof cb !== "function") {
      throw new TypeError("setTimeout requires a function");
    }
    const id = nextTimerId();
    opTimerSleep(coerceDelay(ms)).then(() => runOnce(id, cb, args));
    return id;
  };

  globalThis.clearTimeout = function clearTimeout(id) {
    if (typeof id === "number") cancelled.add(id);
  };

  globalThis.setInterval = function setInterval(cb, ms, ...args) {
    if (typeof cb !== "function") {
      throw new TypeError("setInterval requires a function");
    }
    const id = nextTimerId();
    const delay = coerceDelay(ms);
    const tick = () => {
      if (cancelled.delete(id)) return;
      try { cb(...args); } catch (err) { reportTimerError(err); }
      if (cancelled.has(id)) {
        cancelled.delete(id);
        return;
      }
      opTimerSleep(delay).then(tick);
    };
    opTimerSleep(delay).then(tick);
    return id;
  };

  globalThis.clearInterval = function clearInterval(id) {
    if (typeof id === "number") cancelled.add(id);
  };

  globalThis.setImmediate = function setImmediate(cb, ...args) {
    if (typeof cb !== "function") {
      throw new TypeError("setImmediate requires a function");
    }
    const id = nextTimerId();
    opVoidDeferred().then(() => runOnce(id, cb, args));
    return id;
  };

  globalThis.clearImmediate = function clearImmediate(id) {
    if (typeof id === "number") cancelled.add(id);
  };

  if (typeof globalThis.queueMicrotask !== "function") {
    globalThis.queueMicrotask = (cb) => Promise.resolve().then(cb);
  }

  function reportTimerError(err) {
    try {
      if (typeof globalThis.reportError === "function") {
        globalThis.reportError(err);
      } else if (globalThis.console && typeof globalThis.console.error === "function") {
        globalThis.console.error(err);
      }
    } catch { }
  }

  Object.defineProperty(globalThis, "__nexideTimersInstalled", {
    value: true,
    enumerable: false,
    configurable: false,
    writable: false,
  });
})(globalThis);
