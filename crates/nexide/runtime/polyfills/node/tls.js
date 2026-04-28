"use strict";

/**
 * Polyfill for Node.js `node:tls`.
 *
 * Outbound `connect` performs a real rustls handshake against the
 * `host:port` target through `op_tls_*` ops. The returned
 * `TLSSocket` mirrors the `node:net` `Socket` surface so both can
 * be used interchangeably by client libraries (`node:http2` falls
 * back here, `https` builds on top of it, …).
 *
 * Server-side TLS (`createServer`) is intentionally unavailable in this
 * runtime — terminate TLS at the Rust shield in front of nexide. Calling
 * `tls.createServer()` throws `Error { code: "ERR_NOT_AVAILABLE" }`.
 */

const EventEmitter = require("node:events");
const ops = Nexide.core.ops;

class TLSSocket extends EventEmitter {
  constructor() {
    super();
    this._id = 0;
    this._readable = false;
    this._writable = false;
    this._flowing = false;
    this._paused = false;
    this._pumping = false;
    this.bytesRead = 0;
    this.bytesWritten = 0;
    this.destroyed = false;
    this.authorized = true;
    this.authorizationError = null;
    this.on("newListener", (event) => {
      if (event === "data" && !this._flowing) this.resume();
    });
  }

  _adoptHandle({ id, local, remote }) {
    this._id = id;
    this._readable = true;
    this._writable = true;
    this.localAddress = local && local.address;
    this.localPort = local && local.port;
    this.remoteAddress = remote && remote.address;
    this.remotePort = remote && remote.port;
    queueMicrotask(() => this.emit("secureConnect"));
    if (this._flowing) this._pump();
  }

  write(data, _enc, cb) {
    if (typeof _enc === "function") { cb = _enc; }
    if (!this._id || !this._writable) {
      const err = new Error("tls socket is not writable");
      err.code = "ERR_STREAM_DESTROYED";
      if (cb) queueMicrotask(() => cb(err)); else this.emit("error", err);
      return false;
    }
    let bytes;
    if (data instanceof Uint8Array) bytes = data;
    else if (typeof data === "string") bytes = new TextEncoder().encode(data);
    else if (Buffer && Buffer.isBuffer && Buffer.isBuffer(data)) {
      bytes = new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
    } else throw new TypeError("tls.TLSSocket#write: unsupported data");
    this.bytesWritten += bytes.byteLength;
    ops.op_tls_write(this._id, bytes).then(
      () => { if (cb) cb(null); },
      (err) => { this._writable = false; if (cb) cb(err); this.emit("error", err); },
    );
    return true;
  }

  end(data, enc, cb) {
    if (data) this.write(data, enc);
    this._writable = false;
    queueMicrotask(() => { this.destroy(); if (typeof cb === "function") cb(); });
    return this;
  }

  destroy(err) {
    if (this.destroyed) return this;
    this.destroyed = true;
    this._readable = false;
    this._writable = false;
    if (this._id) { ops.op_tls_close(this._id); this._id = 0; }
    if (err) this.emit("error", err);
    queueMicrotask(() => this.emit("close", !!err));
    return this;
  }

  pause() { this._paused = true; this._flowing = false; return this; }
  resume() { this._paused = false; this._flowing = true; if (this._id) this._pump(); return this; }

  _pump() {
    if (this._pumping || !this._id || this._paused) return;
    this._pumping = true;
    const tick = () => {
      if (!this._id || this._paused) { this._pumping = false; return; }
      ops.op_tls_read(this._id, 65536).then(
        (chunk) => {
          if (!this._id) { this._pumping = false; return; }
          if (chunk.byteLength === 0) {
            this._readable = false;
            this._pumping = false;
            this.emit("end");
            this.destroy();
            return;
          }
          this.bytesRead += chunk.byteLength;
          this.emit("data", Buffer.from(chunk));
          tick();
        },
        (err) => { this._pumping = false; this._readable = false; this.emit("error", err); this.destroy(err); },
      );
    };
    tick();
  }

  setKeepAlive() { return this; }
  setNoDelay() { return this; }
  setTimeout(ms, cb) {
    if (typeof cb === "function") this.once("timeout", cb);
    if (ms > 0) {
      const id = setTimeout(() => this.emit("timeout"), ms);
      this.once("close", () => clearTimeout(id));
    }
    return this;
  }
  ref() { return this; }
  unref() { return this; }
  getPeerCertificate() { return {}; }
  getProtocol() { return "TLSv1.3"; }
}

function connect(...args) {
  let opts = {};
  let cb = null;
  if (typeof args[0] === "object" && args[0] !== null) {
    opts = args[0];
    cb = typeof args[1] === "function" ? args[1] : null;
  } else {
    const port = args.find((a) => typeof a === "number");
    const host = args.find((a) => typeof a === "string");
    cb = args.find((a) => typeof a === "function") || null;
    opts = { port, host };
  }
  const host = opts.host || opts.servername || "127.0.0.1";
  const port = opts.port;
  if (typeof port !== "number") throw new TypeError("tls.connect: port required");
  const sock = new TLSSocket();
  if (cb) sock.once("secureConnect", cb);
  ops.op_tls_connect(String(host), port).then(
    (handle) => sock._adoptHandle(handle),
    (err) => { sock.destroyed = true; sock.emit("error", err); },
  );
  return sock;
}

function createServer() {
  const err = new Error(
    "tls.createServer is not available in nexide; terminate TLS at the Rust shield in front of the runtime",
  );
  err.code = "ERR_NOT_AVAILABLE";
  throw err;
}

module.exports = {
  connect,
  createServer,
  createSecureContext: () => ({}),
  TLSSocket,
  Server: class Server {},
  rootCertificates: [],
  DEFAULT_CIPHERS: "TLS_AES_128_GCM_SHA256:TLS_AES_256_GCM_SHA384:TLS_CHACHA20_POLY1305_SHA256",
  DEFAULT_ECDH_CURVE: "auto",
};
