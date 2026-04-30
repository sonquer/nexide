"use strict";

function noop() {}

function makeConsole(stdout, stderr) {
  var out = stdout || null;
  var err = stderr || stdout || null;
  function write(stream, args) {
    if (!stream || typeof stream.write !== "function") return;
    try {
      var parts = [];
      for (var i = 0; i < args.length; i++) {
        var a = args[i];
        parts.push(typeof a === "string" ? a : String(a));
      }
      stream.write(parts.join(" ") + "\n");
    } catch (_) {
      // swallow - console must never throw
    }
  }
  var fallback = globalThis.console;
  function bind(name, target) {
    if (out || err) {
      return function () {
        write(target === "err" ? err : out, arguments);
      };
    }
    if (fallback && typeof fallback[name] === "function") {
      return fallback[name].bind(fallback);
    }
    return noop;
  }
  return {
    log: bind("log", "out"),
    info: bind("info", "out"),
    debug: bind("debug", "out"),
    warn: bind("warn", "err"),
    error: bind("error", "err"),
    trace: bind("trace", "err"),
    dir: bind("dir", "out"),
    dirxml: bind("log", "out"),
    table: bind("table", "out"),
    group: bind("group", "out"),
    groupCollapsed: bind("group", "out"),
    groupEnd: noop,
    assert: function (cond) {
      if (!cond) {
        var rest = Array.prototype.slice.call(arguments, 1);
        rest.unshift("Assertion failed:");
        write(err, rest);
      }
    },
    count: noop,
    countReset: noop,
    time: noop,
    timeEnd: noop,
    timeLog: noop,
    profile: noop,
    profileEnd: noop,
    timeStamp: noop,
    clear: noop,
  };
}

function Console(options) {
  if (!(this instanceof Console)) {
    return new Console(options);
  }
  var stdout, stderr;
  if (options && typeof options.write === "function") {
    stdout = options;
    stderr = options;
  } else if (options && typeof options === "object") {
    stdout = options.stdout || null;
    stderr = options.stderr || options.stdout || null;
  }
  var c = makeConsole(stdout, stderr);
  for (var k in c) {
    this[k] = c[k];
  }
}

module.exports = globalThis.console;
module.exports.Console = Console;
module.exports.default = globalThis.console;
