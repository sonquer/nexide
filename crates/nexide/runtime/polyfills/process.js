// Node.js `process` polyfill backed by the nexide process op family.
//
// Idempotent - re-running the script (e.g. between tests booting the
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

  function memoryUsage() {
    return typeof ops.op_process_memory_usage === "function"
      ? ops.op_process_memory_usage()
      : { rss: 0, heapTotal: 0, heapUsed: 0, external: 0, arrayBuffers: 0 };
  }
  memoryUsage.rss = () => memoryUsage().rss;

  function cpuUsage(prev) {
    const cur = typeof ops.op_process_cpu_usage === "function"
      ? ops.op_process_cpu_usage()
      : { user: 0, system: 0 };
    if (prev && typeof prev === "object") {
      return {
        user: (cur.user | 0) - ((prev.user | 0) || 0),
        system: (cur.system | 0) - ((prev.system | 0) || 0),
      };
    }
    return cur;
  }

  function killFn(pid, signal) {
    if (typeof ops.op_process_kill !== "function") {
      const err = new Error("process.kill is not supported in this build");
      err.code = "ENOSYS";
      throw err;
    }
    const sig = typeof signal === "number"
      ? signal
      : (signalNumberFor(signal) ?? 15);
    return ops.op_process_kill(pid | 0, sig | 0);
  }

  const SIGNAL_NUMBERS = {
    SIGHUP: 1, SIGINT: 2, SIGQUIT: 3, SIGILL: 4, SIGTRAP: 5,
    SIGABRT: 6, SIGBUS: 7, SIGFPE: 8, SIGKILL: 9, SIGUSR1: 10,
    SIGSEGV: 11, SIGUSR2: 12, SIGPIPE: 13, SIGALRM: 14, SIGTERM: 15,
    SIGCHLD: 17, SIGCONT: 18, SIGSTOP: 19, SIGTSTP: 20, SIGTTIN: 21,
    SIGTTOU: 22, SIGURG: 23, SIGXCPU: 24, SIGXFSZ: 25, SIGVTALRM: 26,
    SIGPROF: 27, SIGWINCH: 28, SIGIO: 29, SIGPWR: 30, SIGSYS: 31,
  };
  function signalNumberFor(name) {
    if (name == null) return SIGNAL_NUMBERS.SIGTERM;
    const upper = String(name).toUpperCase();
    return SIGNAL_NUMBERS[upper];
  }

  function abort() {
    ops.op_process_exit(134);
  }

  function emptyArray() { return []; }
  function alwaysFalse() { return false; }
  function noop() { /* no-op */ }

  const reportStub = {
    directory: "",
    filename: "",
    compact: false,
    excludeNetwork: true,
    signal: "SIGUSR2",
    reportOnFatalError: false,
    reportOnSignal: false,
    reportOnUncaughtException: false,
    writeReport() {
      const err = new Error("process.report.writeReport is not supported in nexide");
      err.code = "ENOSYS";
      throw err;
    },
    getReport() { return {}; },
  };

  const allowedFlags = new Set();
  Object.freeze(allowedFlags);

  let titleState = "nexide";

  const cwdState = { value: meta.cwd };

  const process = {
    __nexideProcess: true,
    env,
    argv: meta.argv,
    argv0: meta.argv[0] || "nexide",
    execPath: meta.argv[0] || "nexide",
    execArgv: [],
    platform: meta.platform,
    arch: meta.arch,
    pid: meta.pid,
    ppid: 0,
    get title() { return titleState; },
    set title(v) { titleState = String(v); },
    version: "v" + meta.version,
    versions: {
      node: "20.18.0",
      nexide: meta.version,
      v8: (typeof Nexide !== "undefined" && Nexide.core && Nexide.core.v8Version)
        ? Nexide.core.v8Version()
        : "unknown",
    },
    config: {
      target_defaults: { default_configuration: "Release" },
      variables: {},
    },
    release: {
      name: "node",
      lts: undefined,
      sourceUrl: "",
      headersUrl: "",
    },
    features: {
      inspector: false,
      debug: false,
      uv: false,
      ipv6: true,
      tls_alpn: true,
      tls_sni: true,
      tls_ocsp: false,
      tls: true,
      cached_builtins: true,
    },
    allowedNodeEnvironmentFlags: allowedFlags,
    sourceMapsEnabled: false,
    setSourceMapsEnabled(v) { this.sourceMapsEnabled = Boolean(v); },
    throwDeprecation: false,
    traceDeprecation: false,
    noDeprecation: false,
    connected: false,
    channel: undefined,
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
      const n = Number(code);
      const safe = Number.isFinite(n) ? Math.trunc(n) : 0;
      const clamped = safe >= 0 && safe <= 255 ? safe : 1;
      ops.op_process_exit(clamped);
    },
    abort,
    kill: killFn,
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
    listeners: emptyArray,
    rawListeners: emptyArray,
    listenerCount: () => 0,
    eventNames: emptyArray,
    setMaxListeners: () => process,
    getMaxListeners: () => 10,
    emit: alwaysFalse,
    binding() {
      throw new Error("process.binding is not supported by nexide");
    },
    dlopen(module, filename, _flags) {
      if (!module || typeof module !== "object") {
        const err = new TypeError("process.dlopen: module must be an object");
        err.code = "ERR_INVALID_ARG_TYPE";
        throw err;
      }
      if (typeof filename !== "string" || filename.length === 0) {
        const err = new TypeError("process.dlopen: filename must be a non-empty string");
        err.code = "ERR_INVALID_ARG_TYPE";
        throw err;
      }
      module.exports = ops.op_napi_load(filename);
    },
    umask() { return 0; },
    geteuid: () => 0,
    getegid: () => 0,
    getuid: () => 0,
    getgid: () => 0,
    getgroups: emptyArray,
    setuid: noop,
    setgid: noop,
    setegid: noop,
    seteuid: noop,
    setgroups: noop,
    initgroups: noop,
    memoryUsage,
    cpuUsage,
    resourceUsage() {
      const m = memoryUsage();
      const c = cpuUsage();
      return {
        userCPUTime: c.user || 0,
        systemCPUTime: c.system || 0,
        maxRSS: Math.floor((m.rss || 0) / 1024),
        sharedMemorySize: 0,
        unsharedDataSize: 0,
        unsharedStackSize: 0,
        minorPageFault: 0,
        majorPageFault: 0,
        swappedOut: 0,
        fsRead: 0,
        fsWrite: 0,
        ipcSent: 0,
        ipcReceived: 0,
        signalsCount: 0,
        voluntaryContextSwitches: 0,
        involuntaryContextSwitches: 0,
      };
    },
    constrainedMemory() { return 0; },
    availableMemory() { return 0; },
    getActiveResourcesInfo: emptyArray,
    hasUncaughtExceptionCaptureCallback: alwaysFalse,
    setUncaughtExceptionCaptureCallback: noop,
    send: undefined,
    disconnect: noop,
    report: reportStub,
    uptime: () => Number(ops.op_process_hrtime_ns() / 1_000_000_000n),
    loadEnvFile(maybePath) {
      const fs = globalThis.require ? globalThis.require("node:fs") : null;
      if (!fs) {
        const err = new Error("process.loadEnvFile requires node:fs");
        err.code = "ENOSYS";
        throw err;
      }
      const target = maybePath || ".env";
      const text = fs.readFileSync(target, "utf8");
      for (const rawLine of String(text).split(/\r?\n/)) {
        const line = rawLine.trim();
        if (!line || line.startsWith("#")) continue;
        const eq = line.indexOf("=");
        if (eq <= 0) continue;
        const key = line.slice(0, eq).trim();
        let value = line.slice(eq + 1).trim();
        if ((value.startsWith("\"") && value.endsWith("\""))
            || (value.startsWith("'") && value.endsWith("'"))) {
          value = value.slice(1, -1);
        }
        if (!(key in env)) env[key] = value;
      }
    },
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
