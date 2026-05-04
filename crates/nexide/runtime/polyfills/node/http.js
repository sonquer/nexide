"use strict";

// node:http - full Node-shaped surface backed by the `__nexide`
// handler stack on the server side and by the `op_http_*` host ops
// on the client side.
//
// **Server**: `Server` registers a function with
// `globalThis.__nexide.pushHandler` at `listen()` time; `close()`
// pops it. Only the top-of-stack handler receives traffic - fresh
// listeners preempt older ones, and closing the top one hands
// traffic back to the previous server (LIFO). The Rust shield
// never opens a real socket on behalf of the JS code; `address()`
// reflects the requested port purely for parity with diagnostics
// that read `server.address()` (Next.js' boot banner does).
//
// `IncomingMessage` extends `node:stream`'s `Readable` and forwards
// the synthetic `data`/`end` events emitted by the dispatcher.
// `ServerResponse` extends `Writable`, buffers headers in a
// case-insensitive map, and proxies through to the synthetic
// `writeHead`/`write`/`end` ops.
//
// **Client**: `request(opts, cb)` and `get(opts, cb)` return a
// [`ClientRequest`] (Writable). When the body is fully buffered
// the request is dispatched through `op_http_request`; the
// resolved descriptor is wrapped into an [`IncomingResponse`]
// (Readable) emitted via the `response` event. Body chunks are
// pulled from the host through `op_http_response_read` until
// end-of-stream.

const EventEmitter = require("node:events");
const { Readable, Writable, Duplex } = require("node:stream");
const { Buffer } = require("node:buffer");

const UPGRADE_SOCKET_ID_HEADER = "x-nexide-upgrade-socket-id";

// Node-shaped raw TCP socket exposed to `'upgrade'` event listeners.
//
// Lifecycle:
//
//   - **pre-handshake**: the JS upgrade listener (typically the
//     `ws` library's `WebSocketServer.handleUpgrade`) writes a
//     complete HTTP/1.1 status + headers blob to the socket. We
//     accumulate those bytes, parse them once `\r\n\r\n` arrives,
//     and commit them as a real `synthRes.writeHead` + `synthRes.end`
//     so the Rust shield emits the 101 on the wire. That 101 flush
//     is what causes hyper's `OnUpgrade` to resolve and the Rust
//     side to attach the upgraded TCP stream into the socket
//     registry.
//
//   - **post-handshake**: subsequent `socket.write()` calls forward
//     bytes through `op_upgrade_socket_write_async`, which queues
//     them onto the upgraded stream (with a Rust-side buffer in
//     case `attach_upgraded` has not run yet). The reader pump
//     continuously calls `op_upgrade_socket_read_async`, parking
//     in Rust until the upgrade has resolved.
//
// The Duplex superclass takes care of `'data'` listener
// dispatching, backpressure, `pipe()`, and graceful `end()`.
class UpgradeSocket extends Duplex {
  constructor(socketId, synthRes) {
    super({
      allowHalfOpen: true,
      write: (chunk, enc, cb) => this._write(chunk, enc, cb),
    });
    this._socketId = socketId;
    this._synthRes = synthRes;
    this._state = "pre-handshake";
    this._headBuffer = [];
    this._headBufferLen = 0;
    this._pumping = false;
    this.readable = true;
    this.writable = true;
    this.encrypted = false;
    this.remoteAddress = "127.0.0.1";
    this.remotePort = 0;
    this.localAddress = "127.0.0.1";
    this.localPort = 0;
  }

  setNoDelay() { return this; }
  setKeepAlive() { return this; }
  setTimeout(_ms, cb) {
    if (typeof cb === "function") this.once("timeout", cb);
    return this;
  }
  ref() { return this; }
  unref() { return this; }
  address() { return { port: this.localPort, family: "IPv4", address: this.localAddress }; }

  _write(chunk, _enc, cb) {
    if (this._state === "destroyed") {
      cb(new Error("socket has been destroyed"));
      return;
    }
    const buf = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
    if (this._state === "pre-handshake") {
      this._handlePreHandshakeWrite(buf, cb);
      return;
    }
    this._sendPostHandshake(buf, cb);
  }

  _handlePreHandshakeWrite(buf, cb) {
    this._headBuffer.push(buf);
    this._headBufferLen += buf.length;
    const concat = this._headBufferLen === buf.length
      ? buf
      : Buffer.concat(this._headBuffer, this._headBufferLen);
    const headEnd = concat.indexOf("\r\n\r\n");
    if (headEnd < 0) {
      // Soft cap to avoid unbounded buffering on malformed input.
      if (this._headBufferLen > 64 * 1024) {
        cb(new Error("upgrade handshake exceeds 64 KiB without CRLFCRLF"));
        return;
      }
      cb();
      return;
    }
    const headBytes = concat.slice(0, headEnd);
    const tail = concat.slice(headEnd + 4);
    let parsed;
    try {
      parsed = parseHttpResponseHead(headBytes);
    } catch (err) {
      cb(err);
      return;
    }
    try {
      this._synthRes.writeHead(parsed.status, parsed.headers);
      this._synthRes.end();
    } catch (err) {
      cb(err);
      return;
    }
    this._state = "post-handshake";
    this._headBuffer = null;
    this.emit("handshake-committed");
    this._startReaderPump();
    if (tail.length > 0) {
      this._sendPostHandshake(tail, cb);
      return;
    }
    cb();
  }

  _sendPostHandshake(buf, cb) {
    Nexide.core.ops
      .op_upgrade_socket_write_async(this._socketId, buf)
      .then(() => cb(), (err) => cb(err));
  }

  _read() { /* reader pump drives push() */ }

  _startReaderPump() {
    if (this._pumping) return;
    this._pumping = true;
    const loop = async () => {
      while (this._state === "post-handshake") {
        let chunk;
        try {
          chunk = await Nexide.core.ops.op_upgrade_socket_read_async(this._socketId);
        } catch (err) {
          this.destroy(err);
          return;
        }
        if (chunk === null) {
          this._state = "ended";
          this.push(null);
          return;
        }
        if (!this.push(Buffer.from(chunk))) {
          // backpressure — wait for _read to be called again
          await new Promise((resolve) => this.once("drain-resume", resolve));
        }
      }
    };
    loop().catch((err) => this.destroy(err));
  }

  _final(cb) {
    if (this._state === "post-handshake" || this._state === "ended") {
      try { Nexide.core.ops.op_upgrade_socket_close(this._socketId); } catch { /* noop */ }
    }
    this._state = "destroyed";
    cb();
  }

  _destroy(err, cb) {
    if (this._state !== "destroyed") {
      try { Nexide.core.ops.op_upgrade_socket_close(this._socketId); } catch { /* noop */ }
      this._state = "destroyed";
    }
    cb(err);
  }
}

function parseHttpResponseHead(buf) {
  // Accept either a full status line + headers or just headers.
  // `ws` always writes `HTTP/1.1 101 ...` first. We parse the status
  // line if present; otherwise default to 101.
  const text = buf.toString("latin1");
  const lines = text.split("\r\n");
  let statusCode = 101;
  let firstHeaderLine = 0;
  if (lines.length > 0 && /^HTTP\//.test(lines[0])) {
    const m = /^HTTP\/\d\.\d\s+(\d{3})/.exec(lines[0]);
    if (!m) throw new Error("malformed HTTP status line in upgrade write");
    statusCode = Number(m[1]);
    firstHeaderLine = 1;
  }
  const headers = [];
  for (let i = firstHeaderLine; i < lines.length; i++) {
    const line = lines[i];
    if (line === "") continue;
    const idx = line.indexOf(":");
    if (idx < 0) throw new Error(`malformed header in upgrade write: ${line}`);
    const name = line.slice(0, idx).trim().toLowerCase();
    const value = line.slice(idx + 1).trim();
    headers.push([name, value]);
  }
  return { status: statusCode, headers };
}

const STATUS_CODES = {
  100: "Continue",
  101: "Switching Protocols",
  200: "OK",
  201: "Created",
  202: "Accepted",
  204: "No Content",
  301: "Moved Permanently",
  302: "Found",
  303: "See Other",
  304: "Not Modified",
  307: "Temporary Redirect",
  308: "Permanent Redirect",
  400: "Bad Request",
  401: "Unauthorized",
  403: "Forbidden",
  404: "Not Found",
  405: "Method Not Allowed",
  408: "Request Timeout",
  409: "Conflict",
  410: "Gone",
  413: "Payload Too Large",
  415: "Unsupported Media Type",
  418: "I'm a teapot",
  422: "Unprocessable Entity",
  429: "Too Many Requests",
  500: "Internal Server Error",
  501: "Not Implemented",
  502: "Bad Gateway",
  503: "Service Unavailable",
  504: "Gateway Timeout",
};

const METHODS = [
  "ACL", "BIND", "CHECKOUT", "CONNECT", "COPY", "DELETE", "GET", "HEAD",
  "LINK", "LOCK", "M-SEARCH", "MERGE", "MKACTIVITY", "MKCALENDAR", "MKCOL",
  "MOVE", "NOTIFY", "OPTIONS", "PATCH", "POST", "PROPFIND", "PROPPATCH",
  "PURGE", "PUT", "REBIND", "REPORT", "SEARCH", "SOURCE", "SUBSCRIBE",
  "TRACE", "UNBIND", "UNLINK", "UNLOCK", "UNSUBSCRIBE",
];

function toBytes(chunk, encoding) {
  if (chunk == null) return null;
  if (chunk instanceof Uint8Array) return chunk;
  return Buffer.from(String(chunk), encoding || "utf8");
}

class IncomingMessage extends Readable {
  constructor(synth) {
    super({});
    this._synth = synth;
    this.method = synth.method;
    this.url = synth.url;
    this.httpVersion = "1.1";
    this.httpVersionMajor = 1;
    this.httpVersionMinor = 1;
    this.headers = synth.headers;
    this.rawHeaders = synth.rawHeaders;
    this.trailers = Object.create(null);
    this.rawTrailers = [];
    this.complete = false;
    this.socket = {
      remoteAddress: undefined,
      remotePort: undefined,
      setTimeout(_ms, cb) { if (typeof cb === "function") this._timeoutCb = cb; return this; },
      setNoDelay(_enable) { return this; },
      setKeepAlive(_enable, _initialDelay) { return this; },
      ref() { return this; },
      unref() { return this; },
      destroy() {},
      end() {},
    };
    this.connection = this.socket;

    synth.on("data", (chunk) => this.push(chunk));
    synth.on("end", () => {
      this.complete = true;
      this.push(null);
    });
    synth.on("error", (err) => this.destroy(err));
  }
  setTimeout(_ms, cb) {
    if (typeof cb === "function") this.once("timeout", cb);
    return this;
  }
}

class ServerResponse extends Writable {
  constructor(synth) {
    super({});
    this._synth = synth;
    this._headers = new Map();
    this._headersSent = false;
    this._ended = false;
    this._destroyed = false;
    this._writableFinished = false;
    this.statusCode = 200;
    this.statusMessage = "OK";
    this.sendDate = false;
    this.req = null;
  }

  get headersSent() {
    return this._headersSent;
  }

  get writableFinished() {
    return this._writableFinished;
  }

  get writableEnded() {
    return this._ended;
  }

  get destroyed() {
    return this._destroyed;
  }

  setHeader(name, value) {
    if (this._headersSent) {
      const err = new Error("ERR_HTTP_HEADERS_SENT: headers already sent");
      err.code = "ERR_HTTP_HEADERS_SENT";
      throw err;
    }
    this._headers.set(String(name).toLowerCase(), String(value));
    return this;
  }

  appendHeader(name, value) {
    if (this._headersSent) {
      const err = new Error("ERR_HTTP_HEADERS_SENT: headers already sent");
      err.code = "ERR_HTTP_HEADERS_SENT";
      throw err;
    }
    const key = String(name).toLowerCase();
    const existing = this._headers.get(key);
    if (existing === undefined) {
      this._headers.set(key, Array.isArray(value) ? value.map(String) : String(value));
    } else if (Array.isArray(existing)) {
      if (Array.isArray(value)) existing.push(...value.map(String));
      else existing.push(String(value));
    } else {
      const arr = [String(existing)];
      if (Array.isArray(value)) arr.push(...value.map(String));
      else arr.push(String(value));
      this._headers.set(key, arr);
    }
    return this;
  }


  getHeader(name) {
    return this._headers.get(String(name).toLowerCase());
  }

  hasHeader(name) {
    return this._headers.has(String(name).toLowerCase());
  }

  removeHeader(name) {
    if (this._headersSent) {
      const err = new Error("ERR_HTTP_HEADERS_SENT: headers already sent");
      err.code = "ERR_HTTP_HEADERS_SENT";
      throw err;
    }
    this._headers.delete(String(name).toLowerCase());
  }

  getHeaders() {
    const out = Object.create(null);
    for (const [k, v] of this._headers) out[k] = v;
    return out;
  }

  getHeaderNames() {
    return Array.from(this._headers.keys());
  }

  flushHeaders() {
    if (this._headersSent) return;
    this._sendHead();
  }

  _implicitHeader() {
    if (!this._headersSent) this._sendHead();
  }

  _sendHead() {
    const headers = [];
    for (const [k, v] of this._headers) {
      if (Array.isArray(v)) {
        for (const item of v) headers.push([k, String(item)]);
      } else {
        headers.push([k, String(v)]);
      }
    }
    this._synth.writeHead(this.statusCode, headers);
    this._headersSent = true;
    this._header = `HTTP/1.1 ${this.statusCode} ${this.statusMessage}\r\n`;
  }

  writeHead(status, statusMessage, headers) {
    if (this._headersSent) {
      const err = new Error("ERR_HTTP_HEADERS_SENT: headers already sent");
      err.code = "ERR_HTTP_HEADERS_SENT";
      throw err;
    }
    if (typeof statusMessage === "string") {
      this.statusMessage = statusMessage;
    } else if (statusMessage && headers === undefined) {
      headers = statusMessage;
    }
    this.statusCode = status;
    if (headers) {
      if (Array.isArray(headers)) {
        if (headers.length && Array.isArray(headers[0])) {
          for (const pair of headers) this.setHeader(pair[0], pair[1]);
        } else {
          for (let i = 0; i + 1 < headers.length; i += 2) {
            this.setHeader(headers[i], headers[i + 1]);
          }
        }
      } else {
        for (const k of Object.keys(headers)) this.setHeader(k, headers[k]);
      }
    }
    this._sendHead();
    return this;
  }

  writeContinue() {}

  write(chunk, encoding, cb) {
    if (typeof encoding === "function") { cb = encoding; encoding = undefined; }
    if (this._ended) {
      const err = new Error("write after end()");
      if (cb) cb(err); else throw err;
      return false;
    }
    if (!this._headersSent) this._sendHead();
    const buf = toBytes(chunk, encoding);
    if (buf && buf.byteLength > 0) {
      this._synth.write(buf);
    }
    if (cb) queueMicrotask(() => cb());
    return true;
  }

  end(chunk, encoding, cb) {
    if (typeof chunk === "function") { cb = chunk; chunk = undefined; }
    if (typeof encoding === "function") { cb = encoding; encoding = undefined; }
    if (this._ended) {
      if (cb) queueMicrotask(() => cb());
      return this;
    }
    if (!this._headersSent) this._sendHead();
    const buf = toBytes(chunk, encoding);
    if (buf && buf.byteLength > 0) {
      this._synth.write(buf);
    }
    this._synth.end();
    this._ended = true;
    queueMicrotask(() => {
      this._writableFinished = true;
      this.emit("finish");
      this.emit("close");
      if (cb) cb();
    });
    return this;
  }

  destroy(err) {
    if (this._destroyed) return this;
    this._destroyed = true;
    try { this._synth.end(); } catch { }
    this._ended = true;
    queueMicrotask(() => {
      if (err) this.emit("error", err);
      this._writableFinished = true;
      this.emit("close");
    });
    return this;
  }

  setTimeout(_ms, cb) {
    if (typeof cb === "function") this.once("timeout", cb);
    return this;
  }
}

class Server extends EventEmitter {
  constructor(opts, handler) {
    super();
    if (typeof opts === "function") {
      handler = opts;
      opts = {};
    }
    this._opts = opts || {};
    this._token = null;
    this._address = null;
    this._listening = false;
    if (typeof handler === "function") this.on("request", handler);
  }

  listen(...args) {
    let port = 0;
    let host = "0.0.0.0";
    let cb = null;
    for (const a of args) {
      if (typeof a === "function") cb = a;
      else if (typeof a === "number") port = a;
      else if (typeof a === "string") host = a;
      else if (a && typeof a === "object") {
        if (typeof a.port === "number") port = a.port;
        if (typeof a.host === "string") host = a.host;
      }
    }
    this._address = { port, family: "IPv4", address: host };
    this._listening = true;

    const adapter = (synthReq, synthRes) => {
      const req = new IncomingMessage(synthReq);
      const res = new ServerResponse(synthRes);
      res.req = req;
      const upgradeHeader = req.headers && req.headers.upgrade;
      const upgradeSocketIdRaw = req.headers && req.headers[UPGRADE_SOCKET_ID_HEADER];
      const upgradeSocketId = upgradeSocketIdRaw === undefined
        ? null
        : Number(upgradeSocketIdRaw);
      if (upgradeHeader) {
        const upgradeListeners = this.listeners("upgrade");
        if (upgradeListeners.length > 0 && upgradeSocketId !== null
            && Number.isFinite(upgradeSocketId)) {
          // Strip the synthetic header so user code does not see it.
          if (req.headers) delete req.headers[UPGRADE_SOCKET_ID_HEADER];
          if (Array.isArray(req.rawHeaders)) {
            for (let i = req.rawHeaders.length - 2; i >= 0; i -= 2) {
              if (String(req.rawHeaders[i]).toLowerCase() === UPGRADE_SOCKET_ID_HEADER) {
                req.rawHeaders.splice(i, 2);
              }
            }
          }
          const socket = new UpgradeSocket(upgradeSocketId, synthRes);
          req.socket = socket;
          req.connection = socket;
          // The dispatch promise resolves once the user has either
          // committed the 101 (via socket.write of the head) or torn
          // the socket down. We resolve when synthRes finishes so the
          // Rust shield can flush headers; subsequent socket I/O is
          // independent of this promise.
          const ready = new Promise((resolve) => {
            let settled = false;
            const settle = () => { if (!settled) { settled = true; resolve(); } };
            if (typeof synthRes.__isEnded === "function" && synthRes.__isEnded()) {
              settle();
              return;
            }
            socket.once("handshake-committed", settle);
            socket.once("close", settle);
            socket.once("error", settle);
            // Safety net: if no head is written within a long window
            // we still let the response complete so hyper sends a
            // sane status.
            setTimeout(() => {
              if (!settled && !(typeof synthRes.__isEnded === "function" && synthRes.__isEnded())) {
                try {
                  res.statusCode = 500;
                  res.setHeader("Connection", "close");
                  res.end("upgrade listener did not commit a handshake");
                } catch { /* noop */ }
                settle();
              }
            }, 30_000).unref?.();
          });
          for (const fn of upgradeListeners) {
            try { fn(req, socket, Buffer.alloc(0)); } catch (e) { this.emit("error", e); }
          }
          return ready;
        }
        if (upgradeListeners.length > 0) {
          // Listener present but no socket id (e.g. transport that
          // does not support hyper upgrade). Best-effort fallback:
          // emit with a null socket and let the listener decide.
          for (const fn of upgradeListeners) {
            try { fn(req, null, Buffer.alloc(0)); } catch (e) { this.emit("error", e); }
          }
          if (!res._ended && !res._destroyed) {
            res.statusCode = 501;
            res.setHeader("Connection", "close");
            res.end("Upgrade not supported on this connection");
          }
          return Promise.resolve();
        }
        if (this.listenerCount("request") === 0) {
          res.statusCode = 501;
          res.setHeader("Connection", "close");
          res.end("Upgrade not supported by this nexide build");
          return Promise.resolve();
        }
      }
      const listeners = this.listeners("request");
      const pending = [];
      for (const fn of listeners) {
        const ret = fn(req, res);
        if (ret && typeof ret.then === "function") pending.push(ret);
      }
      return Promise.all(pending).then(
        () => new Promise((resolve) => {
          if (res._ended || res._destroyed) { resolve(); return; }
          const done = () => resolve();
          res.once("finish", done);
          res.once("close", done);
        }),
      );
    };
    this._token = globalThis.__nexide.pushHandler(adapter);

    queueMicrotask(() => {
      this.emit("listening");
      if (typeof cb === "function") cb();
    });
    return this;
  }

  close(cb) {
    if (this._token != null) {
      globalThis.__nexide.popHandler(this._token);
      this._token = null;
    }
    this._listening = false;
    queueMicrotask(() => {
      this.emit("close");
      if (typeof cb === "function") cb();
    });
    return this;
  }

  address() {
    return this._address;
  }

  setTimeout(_ms, cb) {
    if (typeof cb === "function") this.once("timeout", cb);
    return this;
  }

  ref() { return this; }
  unref() { return this; }

  get listening() {
    return this._listening;
  }
}

function buildHeaderArray(headers) {
  const out = [];
  if (!headers) return out;
  if (Array.isArray(headers)) {
    for (const entry of headers) {
      if (Array.isArray(entry) && entry.length >= 2) {
        out.push([String(entry[0]), String(entry[1])]);
      }
    }
    return out;
  }
  for (const key of Object.keys(headers)) {
    const value = headers[key];
    if (value === undefined || value === null) continue;
    if (Array.isArray(value)) {
      for (const v of value) out.push([key, String(v)]);
    } else {
      out.push([key, String(value)]);
    }
  }
  return out;
}

function buildRequestUrl(opts, defaultProtocol) {
  if (typeof opts === "string") {
    return { url: opts, protocol: parseProtocol(opts) || defaultProtocol };
  }
  if (opts instanceof URL) {
    return { url: opts.toString(), protocol: opts.protocol };
  }
  const protocol = (opts.protocol || defaultProtocol || "http:").toLowerCase();
  const host = opts.hostname || opts.host || "localhost";
  const port = opts.port ? `:${opts.port}` : "";
  let path = opts.path || "/";
  if (!path.startsWith("/")) path = `/${path}`;
  const auth = opts.auth ? `${opts.auth}@` : "";
  return { url: `${protocol}//${auth}${host}${port}${path}`, protocol };
}

function parseProtocol(str) {
  const m = /^([a-zA-Z][a-zA-Z0-9+\-.]*:)/.exec(str);
  return m ? m[1].toLowerCase() : null;
}

class ClientRequest extends Writable {
  constructor(opts, callback) {
    super();
    this._opts = opts;
    this._headers = buildHeaderArray(opts.headers);
    this._method = (opts.method || "GET").toUpperCase();
    this._chunks = [];
    this._sent = false;
    this._aborted = false;
    if (typeof callback === "function") {
      this.once("response", callback);
    }
    queueMicrotask(() => this._dispatchIfReady());
  }

  setHeader(name, value) {
    this._headers.push([String(name), String(value)]);
  }

  getHeader(name) {
    const lower = String(name).toLowerCase();
    for (const [k, v] of this._headers) {
      if (k.toLowerCase() === lower) return v;
    }
    return undefined;
  }

  removeHeader(name) {
    const lower = String(name).toLowerCase();
    this._headers = this._headers.filter(([k]) => k.toLowerCase() !== lower);
  }

  setTimeout(_ms, cb) {
    if (typeof cb === "function") this.once("timeout", cb);
    return this;
  }

  setNoDelay(_enable) { return this; }

  setSocketKeepAlive(_enable, _initialDelay) { return this; }

  write(chunk, encoding, callback) {
    if (typeof encoding === "function") { callback = encoding; encoding = undefined; }
    if (chunk == null) { if (callback) callback(); return true; }
    let buf;
    if (chunk instanceof Uint8Array) {
      buf = chunk;
    } else if (chunk instanceof ArrayBuffer) {
      buf = new Uint8Array(chunk);
    } else if (ArrayBuffer.isView(chunk)) {
      buf = new Uint8Array(chunk.buffer, chunk.byteOffset, chunk.byteLength);
    } else if (typeof chunk === "string") {
      buf = new TextEncoder().encode(chunk);
    } else {
      buf = new TextEncoder().encode(String(chunk));
    }
    this._chunks.push(buf);
    if (callback) callback();
    return true;
  }

  _write(chunk, _encoding, callback) {
    this.write(chunk);
    callback();
  }

  end(chunk, encoding, callback) {
    if (typeof chunk === "function") { callback = chunk; chunk = undefined; encoding = undefined; }
    else if (typeof encoding === "function") { callback = encoding; encoding = undefined; }
    if (chunk !== undefined && chunk !== null) {
      this.write(chunk, encoding);
    }
    this._ended = true;
    if (callback) callback();
    this._sent = true;
    this._dispatchIfReady();
  }

  abort() {
    this._aborted = true;
    this.emit("abort");
  }

  destroy(err) {
    this._aborted = true;
    if (err) this.emit("error", err);
    super.destroy(err);
  }

  _dispatchIfReady() {
    if (!this._sent || this._aborted || this._dispatched) return;
    this._dispatched = true;
    const total = this._chunks.reduce((acc, b) => acc + b.length, 0);
    const body = new Uint8Array(total);
    let off = 0;
    for (const c of this._chunks) {
      body.set(c, off);
      off += c.length;
    }
    const { url } = buildRequestUrl(this._opts, this._opts.protocol);
    const reqDescriptor = {
      method: this._method,
      url,
      headers: this._headers,
      body: body.length === 0 ? null : body,
    };
    Nexide.core.ops.op_http_request(reqDescriptor).then(
      (resp) => this._onResponse(resp),
      (err) => this.emit("error", err),
    );
  }

  _onResponse(resp) {
    const incoming = new IncomingResponse(resp);
    this.emit("response", incoming);
    incoming._pump();
  }
}

class IncomingResponse extends Readable {
  constructor(resp) {
    super();
    this.statusCode = resp.status;
    this.statusMessage = resp.statusText;
    this.httpVersion = "1.1";
    this.httpVersionMajor = 1;
    this.httpVersionMinor = 1;
    this.headers = {};
    this.rawHeaders = [];
    for (const [name, value] of resp.headers) {
      this.rawHeaders.push(name, value);
      const lower = name.toLowerCase();
      if (this.headers[lower] === undefined) {
        this.headers[lower] = value;
      } else if (Array.isArray(this.headers[lower])) {
        this.headers[lower].push(value);
      } else {
        this.headers[lower] = [this.headers[lower], value];
      }
    }
    this._bodyId = resp.bodyId;
    this._closed = false;
  }

  _read() { /* pump-driven */ }

  async _pump() {
    while (!this._closed) {
      try {
        const chunk = await Nexide.core.ops.op_http_response_read(this._bodyId);
        if (chunk === null) {
          this._closed = true;
          this.push(null);
          Nexide.core.ops.op_http_response_close(this._bodyId);
          this.emit("end");
          return;
        }
        this.push(chunk);
      } catch (err) {
        this._closed = true;
        Nexide.core.ops.op_http_response_close(this._bodyId);
        this.emit("error", err);
        return;
      }
    }
  }

  destroy(err) {
    if (!this._closed) {
      this._closed = true;
      Nexide.core.ops.op_http_response_close(this._bodyId);
    }
    if (err) this.emit("error", err);
    super.destroy(err);
  }
}

function clientRequest(defaultProtocol, opts, callback) {
  if (typeof opts === "string" || opts instanceof URL) {
    const url = opts instanceof URL ? opts : new URL(opts);
    return new ClientRequest({
      protocol: url.protocol,
      hostname: url.hostname,
      port: url.port,
      path: `${url.pathname}${url.search}`,
      method: "GET",
    }, callback);
  }
  return new ClientRequest({ ...opts, protocol: opts.protocol || defaultProtocol }, callback);
}

function clientGet(defaultProtocol, opts, callback) {
  const req = clientRequest(defaultProtocol, opts, callback);
  req.end();
  return req;
}

function createServer(opts, handler) {
  return new Server(opts, handler);
}

class Agent extends EventEmitter {
  constructor(options) {
    super();
    this.options = { ...(options || {}) };
    this.maxSockets = this.options.maxSockets ?? Infinity;
    this.maxTotalSockets = this.options.maxTotalSockets ?? Infinity;
    this.maxFreeSockets = this.options.maxFreeSockets ?? 256;
    this.keepAlive = !!this.options.keepAlive;
    this.keepAliveMsecs = this.options.keepAliveMsecs ?? 1000;
    this.sockets = Object.create(null);
    this.freeSockets = Object.create(null);
    this.requests = Object.create(null);
  }
  destroy() {}
  getName() { return ""; }
  addRequest() {}
  createConnection() { return null; }
  removeSocket() {}
  reuseSocket() {}
  keepSocketAlive() { return false; }
}

const http = {
  createServer,
  Server,
  IncomingMessage,
  ServerResponse,
  ClientRequest,
  Agent,
  globalAgent: new Agent({ keepAlive: false }),
  STATUS_CODES,
  METHODS,
  request: (opts, cb) => clientRequest("http:", opts, cb),
  get: (opts, cb) => clientGet("http:", opts, cb),
  _clientRequest: clientRequest,
  _clientGet: clientGet,
};
http.default = http;

module.exports = http;
