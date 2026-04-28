// Node.js `process` polyfill backed by the nexide process op family.
//
// Idempotent — re-running the script (e.g. between tests booting the
// same isolate) is a no-op so polyfill installation can run more than
// once. All ops live in the `nexide_process_ops` extension; if the
// extension is missing the first lookup throws synchronously.

((globalThis) => {
  "use strict";

  if (globalThis.process && globalThis.process.__nexideProcess) {
    return;
  }

  const ops = Nexide.core.ops;

  const meta = ops.op_process_meta();

  const envHandler = {
    get(_target, prop) {
      if (typeof prop !== "string") return undefined;
      const v = ops.op_process_env_get(prop);
      return v === null || v === undefined ? undefined : v;
    },
    has(_target, prop) {
      if (typeof prop !== "string") return false;
      return ops.op_process_env_has(prop);
    },
    ownKeys() {
      return ops.op_process_env_keys();
    },
    getOwnPropertyDescriptor(_target, prop) {
      if (typeof prop !== "string") return undefined;
      const v = ops.op_process_env_get(prop);
      if (v === null || v === undefined) return undefined;
      return { value: v, writable: true, enumerable: true, configurable: true };
    },
    set(_target, prop, value) {
      if (typeof prop !== "string") return true;
      const v = value === undefined || value === null ? "" : String(value);
      ops.op_process_env_set(prop, v);
      return true;
    },
    deleteProperty(_target, prop) {
      if (typeof prop !== "string") return true;
      ops.op_process_env_delete(prop);
      return true;
    },
  };

  const env = new Proxy(Object.create(null), envHandler);

  const noopEmitter = () => process;

  const stdout = {
    write(chunk) {
      const s = typeof chunk === "string"
        ? chunk
        : new TextDecoder().decode(chunk);
      Nexide.core.print(s);
      return true;
    },
    isTTY: false,
    fd: 1,
  };

  const stderr = {
    write(chunk) {
      const s = typeof chunk === "string"
        ? chunk
        : new TextDecoder().decode(chunk);
      Nexide.core.print(s, true);
      return true;
    },
    isTTY: false,
    fd: 2,
  };

  function hrtime(prev) {
    const ns = ops.op_process_hrtime_ns();
    const seconds = Number(ns / 1_000_000_000n);
    const nanos = Number(ns % 1_000_000_000n);
    if (Array.isArray(prev) && prev.length === 2) {
      let s = seconds - prev[0];
      let n = nanos - prev[1];
      if (n < 0) {
        s -= 1;
        n += 1_000_000_000;
      }
      return [s, n];
    }
    return [seconds, nanos];
  }
  hrtime.bigint = () => ops.op_process_hrtime_ns();

  function nextTick(cb, ...args) {
    if (typeof cb !== "function") {
      throw new TypeError("process.nextTick requires a function");
    }
    queueMicrotask(() => cb(...args));
  }

  const cwdState = { value: meta.cwd };

  const process = {
    __nexideProcess: true,
    env,
    argv: meta.argv,
    argv0: meta.argv[0] || "nexide",
    execPath: meta.argv[0] || "nexide",
    platform: meta.platform,
    arch: meta.arch,
    pid: meta.pid,
    ppid: 0,
    title: "nexide",
    version: "v" + meta.version,
    versions: {
      node: "20.0.0",
      nexide: meta.version,
      v8: (typeof Nexide !== "undefined" && Nexide.core && Nexide.core.v8Version)
        ? Nexide.core.v8Version()
        : "unknown",
    },
    cwd() {
      return cwdState.value;
    },
    chdir(dir) {
      if (typeof dir !== "string" || dir.length === 0) {
        const err = new TypeError(
          "The \"directory\" argument must be of type string.",
        );
        err.code = "ERR_INVALID_ARG_TYPE";
        throw err;
      }
      cwdState.value = dir;
    },
    exit(code) {
      ops.op_process_exit((code | 0) || 0);
    },
    hrtime,
    nextTick,
    stdout,
    stderr,
    stdin: { fd: 0, isTTY: false },
    emitWarning(warning) {
      Nexide.core.print("(warning) " + String(warning) + "\n", true);
    },
    on: noopEmitter,
    once: noopEmitter,
    off: noopEmitter,
    addListener: noopEmitter,
    removeListener: noopEmitter,
    removeAllListeners: noopEmitter,
    prependListener: noopEmitter,
    prependOnceListener: noopEmitter,
    listeners: () => [],
    rawListeners: () => [],
    listenerCount: () => 0,
    eventNames: () => [],
    setMaxListeners: () => process,
    getMaxListeners: () => 10,
    emit: () => false,
    binding() {
      throw new Error("process.binding is not supported by nexide");
    },
    umask: () => 0,
    geteuid: () => 0,
    getegid: () => 0,
    getuid: () => 0,
    getgid: () => 0,
    memoryUsage: () => ({
      rss: 0,
      heapTotal: 0,
      heapUsed: 0,
      external: 0,
      arrayBuffers: 0,
    }),
    uptime: () => Number(ops.op_process_hrtime_ns() / 1_000_000_000n),
    features: {},
  };

  Object.defineProperty(process, "__nexideProcess", {
    value: true,
    enumerable: false,
    configurable: false,
    writable: false,
  });

  globalThis.process = process;
  if (typeof globalThis.global === "undefined") {
    globalThis.global = globalThis;
  }

  if (!globalThis.__nexideErrorTrap) {
    const _origErr = globalThis.console.error;
    const formatError = (err, depth) => {
      if (depth > 5) return `${err && err.message ? err.message : err}`;
      if (!(err instanceof Error)) {
        try { return JSON.stringify(err, Object.getOwnPropertyNames(err || {})); }
        catch (_) { return String(err); }
      }
      let out = `${err.name}: ${err.message}\n${err.stack || ""}`;
      if (err.cause !== undefined && err.cause !== null) {
        out += `\n  caused by: ${formatError(err.cause, depth + 1)}`;
      }
      return out;
    };
    globalThis.console.error = function patched(...args) {
      const out = args.map((a) => {
        if (a && a instanceof Error) return formatError(a, 0);
        if (a && typeof a === "object") {
          try { return JSON.stringify(a, Object.getOwnPropertyNames(a)); }
          catch (_) { return String(a); }
        }
        return String(a);
      });
      _origErr.apply(this, out);
    };
    globalThis.__nexideErrorTrap = true;
  }

  if (!globalThis.__nexideRejectionTrap) {
    if (typeof Nexide !== "undefined" && Nexide.core && typeof Nexide.core.setUnhandledPromiseRejectionHandler === "function") {
      Nexide.core.setUnhandledPromiseRejectionHandler((promise, reason) => {
        const text = reason instanceof Error
          ? `${reason.name}: ${reason.message}\n${reason.stack || ""}${reason.cause ? `\n  caused by: ${reason.cause}` : ""}`
          : String(reason);
        Nexide.core.print(`[nexide:unhandled-rejection] ${text}\n`, true);
        return false;
      });
    }
    globalThis.__nexideRejectionTrap = true;
  }
})(globalThis);
