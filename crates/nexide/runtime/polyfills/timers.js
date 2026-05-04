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
 * uses `op_timer_sleep(0)` so the resume goes through the async-
 * completion pump (a real macrotask) — same ordering as Node's
 * libuv "check" phase. This is critical for Next.js streaming SSR:
 * `createFlightDataInjectionTransformStream` relies on
 * `atLeastOneTask()` (a Promise wrapping `setImmediate`) to let
 * the upstream HTML transform fully drain its microtask-queued
 * chunks BEFORE flight RSC chunks are injected. If `setImmediate`
 * resolves on a microtask (e.g. via `op_void_async_deferred`),
 * flight `<script>self.__next_f.push(...)</script>` chunks splice
 * into the middle of HTML attribute writes (e.g.
 * `<body class="bg-g<script>...</script>ray-100 ...">`).
 *
 * Pending timers are tracked by id in a single `Map<id, {cb,args}>`.
 * Capturing only the numeric `id` in the host-Promise `.then` keeps
 * cancelled callbacks from being retained for the full delay (the
 * Rust `tokio::time::sleep` Promise pins its `.then` body until the
 * delay elapses, even after `clearTimeout`). The implementation is
 * idempotent so the file can safely be evaluated once per isolate.
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
  // `pending` maps live timer ids to their `{ cb, args }` payload.
  //
  // The Rust-side `op_timer_sleep` Promise keeps the `.then` body
  // alive until the requested delay has elapsed - even when the
  // user calls `clearTimeout` shortly after scheduling. If we close
  // over `cb` and `args` directly inside that `.then`, every
  // *cancelled* timer leaks its callback closure (and everything
  // it transitively captures - `req`, `res`, response builders for
  // the Next.js handler watchdog, etc.) until the underlying timer
  // fires. Under load (e.g. 400+ RPS with a 60s watchdog) that
  // amounts to tens of thousands of retained closures and tens to
  // hundreds of MB of live JS heap before V8 can collect.
  //
  // Storing payloads in a side-map and only capturing the small
  // numeric `id` in the `.then` lets `clearTimeout` evict the heavy
  // payload immediately. The Rust timer still resolves at its
  // scheduled time, but at that point the map lookup misses and we
  // exit without retaining anything.
  const pending = new Map();

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

  function runOnce(id) {
    const slot = pending.get(id);
    if (slot === undefined) return;
    pending.delete(id);
    slot.cb(...slot.args);
  }

  function makeTimeout(id) {
    const t = Object.create(timeoutProto);
    t._id = id;
    return t;
  }
  const timeoutProto = {
    [Symbol.toPrimitive](hint) {
      return hint === "string" ? String(this._id) : this._id;
    },
    valueOf() { return this._id; },
    toString() { return String(this._id); },
    ref() { return this; },
    unref() { return this; },
    hasRef() { return true; },
    refresh() { return this; },
    [Symbol.dispose]() { pending.delete(this._id); },
  };

  function idOf(t) {
    if (t == null) return -1;
    if (typeof t === "number") return t;
    if (typeof t === "object" && typeof t._id === "number") return t._id;
    return -1;
  }

  globalThis.setTimeout = function setTimeout(cb, ms, ...args) {
    if (typeof cb !== "function") {
      throw new TypeError("setTimeout requires a function");
    }
    const id = nextTimerId();
    pending.set(id, { cb, args });
    opTimerSleep(coerceDelay(ms)).then(() => runOnce(id));
    return makeTimeout(id);
  };

  globalThis.clearTimeout = function clearTimeout(t) {
    const id = idOf(t);
    if (id >= 0) pending.delete(id);
  };

  globalThis.setInterval = function setInterval(cb, ms, ...args) {
    if (typeof cb !== "function") {
      throw new TypeError("setInterval requires a function");
    }
    const id = nextTimerId();
    const delay = coerceDelay(ms);
    pending.set(id, { cb, args });
    const tick = () => {
      const slot = pending.get(id);
      if (slot === undefined) return;
      try { slot.cb(...slot.args); } catch (err) { reportTimerError(err); }
      // Re-check: the user-provided callback may have called
      // `clearInterval` synchronously, which would have evicted us.
      if (!pending.has(id)) return;
      opTimerSleep(delay).then(tick);
    };
    opTimerSleep(delay).then(tick);
    return makeTimeout(id);
  };

  globalThis.clearInterval = function clearInterval(t) {
    const id = idOf(t);
    if (id >= 0) pending.delete(id);
  };

  globalThis.setImmediate = function setImmediate(cb, ...args) {
    if (typeof cb !== "function") {
      throw new TypeError("setImmediate requires a function");
    }
    const id = nextTimerId();
    pending.set(id, { cb, args });
    opTimerSleep(0).then(() => runOnce(id));
    return makeTimeout(id);
  };

  globalThis.clearImmediate = function clearImmediate(t) {
    const id = idOf(t);
    if (id >= 0) pending.delete(id);
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
