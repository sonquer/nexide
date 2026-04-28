"use strict";

// node:url - self-contained legacy `url` API plus a minimal
// WHATWG-ish URL/URLSearchParams shim. The bare V8 isolate that
// nexide boots does not install the WHATWG URL globals, so this
// module ships the minimum surface that covers the request shapes
// exercised by Next.js standalone.

const URL_REGEX = /^([a-z][a-z0-9+\-.]*:)?(?:\/\/((?:([^/?#@]*)@)?([^/?#:]*)(?::(\d+))?))?([^?#]*)(\?[^#]*)?(#.*)?$/i;

class NexideURL {
  constructor(input, base) {
    let str = String(input);
    if (base !== undefined) {
      str = resolveAgainst(String(base), str);
    }
    const m = URL_REGEX.exec(str);
    if (!m || (!m[1] && base === undefined)) {
      throw new TypeError(`Invalid URL: ${str}`);
    }
    this.protocol = (m[1] || "").toLowerCase();
    const auth = m[3] || "";
    const colon = auth.indexOf(":");
    this.username = colon === -1 ? auth : auth.slice(0, colon);
    this.password = colon === -1 ? "" : auth.slice(colon + 1);
    this.hostname = (m[4] || "").toLowerCase();
    this.port = m[5] || "";
    this.pathname = m[6] || (this.hostname ? "/" : "");
    this.search = m[7] || "";
    this.hash = m[8] || "";
  }
  get host() {
    return this.port ? `${this.hostname}:${this.port}` : this.hostname;
  }
  get origin() {
    return this.hostname ? `${this.protocol}//${this.host}` : "null";
  }
  get href() {
    let out = this.protocol;
    if (this.hostname || this.protocol === "file:") {
      out += "//";
      if (this.username) {
        out += this.username;
        if (this.password) out += ":" + this.password;
        out += "@";
      }
      out += this.host;
    }
    out += this.pathname + this.search + this.hash;
    return out;
  }
  toString() { return this.href; }
  toJSON() { return this.href; }
  get searchParams() {
    if (!this._sp) this._sp = new NexideURLSearchParams(this.search);
    return this._sp;
  }
}

class NexideURLSearchParams {
  constructor(init) {
    this._pairs = [];
    if (init === undefined || init === null || init === "") return;
    if (typeof init === "string") {
      const s = init.startsWith("?") ? init.slice(1) : init;
      if (!s) return;
      for (const part of s.split("&")) {
        const i = part.indexOf("=");
        const k = i === -1 ? decode(part) : decode(part.slice(0, i));
        const v = i === -1 ? "" : decode(part.slice(i + 1));
        this._pairs.push([k, v]);
      }
    } else if (Array.isArray(init)) {
      for (const [k, v] of init) this._pairs.push([String(k), String(v)]);
    } else if (typeof init === "object") {
      for (const k of Object.keys(init)) this._pairs.push([k, String(init[k])]);
    }
  }
  append(k, v) { this._pairs.push([String(k), String(v)]); }
  delete(k) { this._pairs = this._pairs.filter(([key]) => key !== k); }
  get(k) {
    const found = this._pairs.find(([key]) => key === k);
    return found ? found[1] : null;
  }
  getAll(k) { return this._pairs.filter(([key]) => key === k).map(([, v]) => v); }
  has(k) { return this._pairs.some(([key]) => key === k); }
  set(k, v) {
    let replaced = false;
    this._pairs = this._pairs.filter(([key]) => {
      if (key !== k) return true;
      if (!replaced) { replaced = true; return true; }
      return false;
    });
    if (replaced) {
      const i = this._pairs.findIndex(([key]) => key === k);
      this._pairs[i] = [k, String(v)];
    } else {
      this._pairs.push([String(k), String(v)]);
    }
  }
  toString() {
    return this._pairs.map(([k, v]) => `${encode(k)}=${encode(v)}`).join("&");
  }
  *entries() { for (const p of this._pairs) yield p; }
  *keys() { for (const [k] of this._pairs) yield k; }
  *values() { for (const [, v] of this._pairs) yield v; }
  forEach(fn, thisArg) { for (const [k, v] of this._pairs) fn.call(thisArg, v, k, this); }
  [Symbol.iterator]() { return this.entries(); }
}

function decode(s) { try { return decodeURIComponent(s.replace(/\+/g, " ")); } catch { return s; } }
function encode(s) { return encodeURIComponent(s).replace(/%20/g, "+"); }

function resolveAgainst(base, target) {
  if (URL_REGEX.test(target) && URL_REGEX.exec(target)[1]) return target;
  const baseM = URL_REGEX.exec(base);
  if (!baseM || !baseM[1]) throw new TypeError(`Invalid base URL: ${base}`);
  const baseProto = baseM[1];
  const baseAuth = baseM[2] ? `//${baseM[2]}` : "";
  const basePath = baseM[6] || "/";
  if (target.startsWith("//")) return `${baseProto}${target}`;
  if (target.startsWith("/")) return `${baseProto}${baseAuth}${target}`;
  if (target.startsWith("?") || target.startsWith("#")) return `${baseProto}${baseAuth}${basePath}${target}`;
  const stripped = basePath.replace(/[^/]*$/, "");
  return `${baseProto}${baseAuth}${stripped}${target}`;
}

const URLImpl = globalThis.URL || NexideURL;
const URLSearchParamsImpl = globalThis.URLSearchParams || NexideURLSearchParams;

function fileURLToPath(u) {
  const url = u instanceof URLImpl || u instanceof NexideURL ? u : new URLImpl(String(u));
  if (url.protocol !== "file:") {
    throw new TypeError("The URL must be of scheme file");
  }
  let path = decodeURIComponent(url.pathname);
  if (typeof globalThis.process !== "undefined"
    && globalThis.process.platform === "win32") {
    path = path.replace(/^\//, "").replace(/\//g, "\\");
  }
  return path;
}

function pathToFileURL(p) {
  const norm = String(p).replace(/\\/g, "/");
  const prefix = norm.startsWith("/") ? "file://" : "file:///";
  return new URLImpl(prefix + encodeURI(norm));
}

function urlParse(input) {
  const m = URL_REGEX.exec(String(input));
  if (!m) {
    return {
      href: input, protocol: null, slashes: false, auth: null,
      host: null, hostname: null, port: null, pathname: input,
      search: null, query: null, hash: null, path: input,
    };
  }
  const proto = m[1] || null;
  const auth = m[3] || null;
  const host = m[4] || null;
  const port = m[5] || null;
  const pathname = m[6] || "";
  const search = m[7] || null;
  const hash = m[8] || null;
  return {
    href: input,
    protocol: proto,
    slashes: Boolean(proto),
    auth,
    host: host ? (port ? `${host}:${port}` : host) : null,
    hostname: host,
    port,
    pathname,
    search,
    query: search ? search.slice(1) : null,
    hash,
    path: pathname + (search || ""),
  };
}

function urlFormat(obj) {
  if (typeof obj === "string") return obj;
  if (obj instanceof NexideURL || (globalThis.URL && obj instanceof globalThis.URL)) {
    return obj.href;
  }
  let result = "";
  if (obj.protocol) {
    result += obj.protocol;
    if (!obj.protocol.endsWith(":")) result += ":";
    if (obj.slashes !== false) result += "//";
  }
  if (obj.auth) result += obj.auth + "@";
  if (obj.host) result += obj.host;
  else if (obj.hostname) result += obj.hostname + (obj.port ? ":" + obj.port : "");
  if (obj.pathname) result += obj.pathname;
  if (obj.search) result += obj.search;
  else if (obj.query) {
    if (typeof obj.query === "string") result += "?" + obj.query;
    else result += "?" + require("node:querystring").stringify(obj.query);
  }
  if (obj.hash) result += obj.hash;
  return result;
}

function urlResolve(from, to) { return resolveAgainst(from, to); }

module.exports = {
  URL: URLImpl,
  URLSearchParams: URLSearchParamsImpl,
  fileURLToPath,
  pathToFileURL,
  parse: urlParse,
  format: urlFormat,
  resolve: urlResolve,
  Url: function Url() {},
};
