"use strict";

/**
 * Replaces the V8 default `console` (which writes to stdout/stderr)
 * with a bridge that forwards every call through a Rust op. The op
 * decides whether to surface the message based on the running
 * isolate's `WorkerId` and emits via the host tracing subscriber.
 *
 * Levels follow the standard `console` taxonomy and map to numeric
 * values consumed by `op_nexide_log`:
 *
 *   0 — trace
 *   1 — debug
 *   2 — log / info
 *   3 — warn
 *   4 — error
 */
((globalThis) => {
  const { ops } = Nexide.core;
  const log = ops.op_nexide_log;

  const formatValue = (value) => {
    if (typeof value === "string") {
      return value;
    }
    if (value instanceof Error) {
      return value.stack || `${value.name}: ${value.message}`;
    }
    if (value === undefined) {
      return "undefined";
    }
    if (value === null) {
      return "null";
    }
    try {
      return JSON.stringify(value);
    } catch {
      return String(value);
    }
  };

  const format = (args) => args.map(formatValue).join(" ");

  const emit = (level) => (...args) => {
    log(level, format(args));
  };

  const noop = () => {};

  globalThis.console = {
    trace: emit(0),
    debug: emit(1),
    log: emit(2),
    info: emit(2),
    dir: (value) => log(2, formatValue(value)),
    dirxml: (value) => log(2, formatValue(value)),
    warn: emit(3),
    error: emit(4),
    table: (value) => log(2, formatValue(value)),
    assert: (condition, ...args) => {
      if (!condition) {
        log(4, "Assertion failed: " + format(args));
      }
    },
    group: noop,
    groupCollapsed: noop,
    groupEnd: noop,
    time: noop,
    timeEnd: noop,
    timeLog: noop,
    count: noop,
    countReset: noop,
    clear: noop,
    profile: noop,
    profileEnd: noop,
  };
})(globalThis);
