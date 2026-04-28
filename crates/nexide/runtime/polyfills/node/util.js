"use strict";

// node:util — minimal but compatible API used by Next.js + commonly
// imported by middleware libraries.

function format(fmt, ...args) {
  if (typeof fmt !== "string") {
    return [fmt, ...args].map(inspect).join(" ");
  }
  let i = 0;
  let out = "";
  for (let j = 0; j < fmt.length; j++) {
    const c = fmt[j];
    if (c === "%" && j + 1 < fmt.length) {
      const next = fmt[++j];
      if (i >= args.length) { out += "%" + next; continue; }
      const a = args[i++];
      switch (next) {
        case "s": out += String(a); break;
        case "d":
        case "i": out += String(Math.trunc(Number(a))); break;
        case "f": out += String(Number(a)); break;
        case "j": try { out += JSON.stringify(a); } catch { out += "[Circular]"; } break;
        case "o":
        case "O": out += inspect(a); break;
        case "%": out += "%"; i--; break;
        default: out += "%" + next; i--;
      }
    } else {
      out += c;
    }
  }
  for (; i < args.length; i++) out += " " + inspect(args[i]);
  return out;
}

function inspect(value, depth) {
  const seen = new WeakSet();
  const max = typeof depth === "number" ? depth : 2;
  function go(v, d) {
    if (v === null) return "null";
    if (v === undefined) return "undefined";
    const t = typeof v;
    if (t === "string") return JSON.stringify(v);
    if (t === "number" || t === "boolean" || t === "bigint" || t === "symbol") return String(v);
    if (t === "function") return `[Function: ${v.name || "anonymous"}]`;
    if (v instanceof Date) return v.toISOString();
    if (v instanceof RegExp) return v.toString();
    if (v instanceof Error) return `${v.name}: ${v.message}`;
    if (typeof v !== "object") return String(v);
    if (seen.has(v)) return "[Circular]";
    seen.add(v);
    if (d < 0) return Array.isArray(v) ? "[Array]" : "[Object]";
    if (Array.isArray(v)) return "[ " + v.map((x) => go(x, d - 1)).join(", ") + " ]";
    const keys = Object.keys(v);
    return "{ " + keys.map((k) => `${k}: ${go(v[k], d - 1)}`).join(", ") + " }";
  }
  return go(value, max);
}

function promisify(fn) {
  if (typeof fn !== "function") {
    throw new TypeError("util.promisify requires a function");
  }
  return function (...args) {
    return new Promise((resolve, reject) => {
      fn.call(this, ...args, (err, result) => {
        if (err) reject(err);
        else resolve(result);
      });
    });
  };
}

function callbackify(fn) {
  if (typeof fn !== "function") {
    throw new TypeError("util.callbackify requires a function");
  }
  return function (...args) {
    const cb = args.pop();
    if (typeof cb !== "function") throw new TypeError("Last argument must be a callback");
    Promise.resolve()
      .then(() => fn.apply(this, args))
      .then((value) => cb(null, value), (err) => cb(err || new Error("Promise rejected")));
  };
}

function inherits(ctor, superCtor) {
  if (ctor === undefined || ctor === null) throw new TypeError("ctor is required");
  if (superCtor === undefined || superCtor === null) throw new TypeError("superCtor is required");
  Object.defineProperty(ctor, "super_", { value: superCtor });
  Object.setPrototypeOf(ctor.prototype, superCtor.prototype);
}

function isDeepStrictEqual(a, b) {
  if (a === b) return true;
  if (a === null || b === null || typeof a !== "object" || typeof b !== "object") {
    return Number.isNaN(a) && Number.isNaN(b);
  }
  if (Object.getPrototypeOf(a) !== Object.getPrototypeOf(b)) return false;
  if (Array.isArray(a)) {
    if (!Array.isArray(b) || a.length !== b.length) return false;
    for (let i = 0; i < a.length; i++) if (!isDeepStrictEqual(a[i], b[i])) return false;
    return true;
  }
  const ka = Object.keys(a);
  const kb = Object.keys(b);
  if (ka.length !== kb.length) return false;
  for (const k of ka) if (!isDeepStrictEqual(a[k], b[k])) return false;
  return true;
}

const types = {
  isDate: (v) => v instanceof Date,
  isRegExp: (v) => v instanceof RegExp,
  isMap: (v) => v instanceof Map,
  isSet: (v) => v instanceof Set,
  isPromise: (v) => v instanceof Promise,
  isArrayBuffer: (v) => v instanceof ArrayBuffer,
  isTypedArray: (v) => ArrayBuffer.isView(v) && !(v instanceof DataView),
  isUint8Array: (v) => v instanceof Uint8Array,
  isNativeError: (v) => v instanceof Error,
  isAsyncFunction: (v) => typeof v === "function" && v.constructor && v.constructor.name === "AsyncFunction",
};

module.exports = {
  format,
  inspect,
  promisify,
  callbackify,
  inherits,
  isDeepStrictEqual,
  types,
  TextEncoder: globalThis.TextEncoder,
  TextDecoder: globalThis.TextDecoder,
  deprecate: (fn) => fn,
};
