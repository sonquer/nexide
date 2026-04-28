"use strict";

/**
 * Polyfill for Node.js `node:net`.
 *
 * `Socket` and `Server` wrap host-side handle ids minted by the
 * `op_net_*` ops. The Rust side owns the actual `tokio::net`
 * resources; JavaScript only ever sees an opaque numeric id.
 *
 * Both classes extend `EventEmitter` and emit the events Node
 * users expect: `connect`, `data`, `end`, `close`, `error` for
 * sockets; `listening`, `connection`, `close`, `error` for servers.
 *
 * Reads are driven by an internal pump: once the socket transitions
 * to flowing mode (a `data` listener is attached, `resume()` is
 * called, …) the pump keeps issuing `op_net_read` until the peer
 * closes the connection or `pause()` is called.
 */

const EventEmitter = require("node:events");
const ops = Nexide.core.ops;

function netError(code, message) {
  const err = new Error(message);
  err.code = code;
  return err;
}

class Socket extends EventEmitter {
  constructor(opts) {
    super();
    this._id = (opts && typeof opts.id === "number") ? opts.id : 0;
    this._readable = !!this._id;
    this._writable = !!this._id;
    this._flowing = false;
    this._paused = false;
    this._pumping = false;
    this.bytesRead = 0;
    this.bytesWritten = 0;
    this.connecting = false;
    this.destroyed = false;
    this.remoteAddress = (opts && opts.remote && opts.remote.address) || undefined;
    this.remotePort = (opts && opts.remote && opts.remote.port) || undefined;
    this.remoteFamily = (opts && opts.remote && opts.remote.family) || undefined;
    this.localAddress = (opts && opts.local && opts.local.address) || undefined;
    this.localPort = (opts && opts.local && opts.local.port) || undefined;
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
    this.remoteFamily = remote && remote.family;
    this.connecting = false;
    queueMicrotask(() => this.emit("connect"));
    if (this._flowing) this._pump();
  }

  connect(...args) {
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
    const host = opts.host || "127.0.0.1";
    const port = opts.port;
    if (typeof port !== "number") {
      throw netError("ERR_INVALID_ARG_TYPE", "port must be a number");
    }
    this.connecting = true;
    if (cb) this.once("connect", cb);
    ops.op_net_connect(String(host), port).then(
      (handle) => this._adoptHandle(handle),
      (err) => {
        this.connecting = false;
        this.destroyed = true;
        this.emit("error", err);
      },
    );
    return this;
  }

  write(data, encoding, cb) {
    if (typeof encoding === "function") { cb = encoding; encoding = undefined; }
    if (!this._id || !this._writable) {
      const err = netError("ERR_STREAM_DESTROYED", "socket is not writable");
      if (cb) queueMicrotask(() => cb(err));
      else this.emit("error", err);
      return false;
    }
    let bytes;
    if (data instanceof Uint8Array) bytes = data;
    else if (typeof data === "string") bytes = new TextEncoder().encode(data);
    else if (Buffer && Buffer.isBuffer && Buffer.isBuffer(data)) bytes = new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
    else throw new TypeError("net.Socket#write: unsupported data");
    this.bytesWritten += bytes.byteLength;
    ops.op_net_write(this._id, bytes).then(
      () => { if (cb) cb(null); },
      (err) => {
        this._writable = false;
        if (cb) cb(err);
        this.emit("error", err);
      },
    );
    return true;
  }

  end(data, encoding, cb) {
    if (data) this.write(data, encoding);
    this._writable = false;
    queueMicrotask(() => {
      this.destroy();
      if (typeof cb === "function") cb();
    });
    return this;
  }

  destroy(err) {
    if (this.destroyed) return this;
    this.destroyed = true;
    this._readable = false;
    this._writable = false;
    if (this._id) {
      ops.op_net_close_stream(this._id);
      this._id = 0;
    }
    if (err) this.emit("error", err);
    queueMicrotask(() => this.emit("close", !!err));
    return this;
  }

  pause() {
    this._paused = true;
    this._flowing = false;
    return this;
  }

  resume() {
    this._paused = false;
    this._flowing = true;
    if (this._id) this._pump();
    return this;
  }

  _pump() {
    if (this._pumping || !this._id || this._paused) return;
    this._pumping = true;
    const tick = () => {
      if (!this._id || this._paused) {
        this._pumping = false;
        return;
      }
      ops.op_net_read(this._id, 65536).then(
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
          const buf = Buffer.from(chunk);
          this.emit("data", buf);
          tick();
        },
        (err) => {
          this._pumping = false;
          this._readable = false;
          this.emit("error", err);
          this.destroy(err);
        },
      );
    };
    tick();
  }

  setKeepAlive(enable, _ms) {
    if (this._id) ops.op_net_set_keepalive(this._id, !!enable);
    return this;
  }

  setNoDelay(enable) {
    if (this._id) ops.op_net_set_nodelay(this._id, enable !== false);
    return this;
  }

  setTimeout(ms, cb) {
    if (typeof cb === "function") this.once("timeout", cb);
    if (ms > 0) {
      const id = setTimeout(() => this.emit("timeout"), ms);
      this.once("close", () => clearTimeout(id));
    }
    return this;
  }

  address() {
    return {
      address: this.localAddress || "",
      port: this.localPort || 0,
      family: "IPv" + ((this.remoteFamily) || 4),
    };
  }

  ref() { return this; }
  unref() { return this; }
  get readable() { return this._readable; }
  get writable() { return this._writable; }
}

class Server extends EventEmitter {
  constructor(opts, handler) {
    super();
    if (typeof opts === "function") { handler = opts; opts = {}; }
    if (typeof handler === "function") this.on("connection", handler);
    this._id = 0;
    this._listening = false;
    this._address = null;
    this._accepting = false;
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
    if (cb) this.once("listening", cb);
    ops.op_net_listen(String(host), port).then(
      ({ id, address }) => {
        this._id = id;
        this._listening = true;
        this._address = { address: address.address, port: address.port, family: "IPv" + address.family };
        this.emit("listening");
        this._acceptLoop();
      },
      (err) => this.emit("error", err),
    );
    return this;
  }

  _acceptLoop() {
    if (this._accepting || !this._id) return;
    this._accepting = true;
    const tick = () => {
      if (!this._id) { this._accepting = false; return; }
      ops.op_net_accept(this._id).then(
        (handle) => {
          if (!this._id) { this._accepting = false; return; }
          const sock = new Socket({});
          sock._adoptHandle(handle);
          this.emit("connection", sock);
          tick();
        },
        (err) => {
          this._accepting = false;
          if (this._id) this.emit("error", err);
        },
      );
    };
    tick();
  }

  close(cb) {
    if (typeof cb === "function") this.once("close", cb);
    if (this._id) {
      ops.op_net_close_listener(this._id);
      this._id = 0;
    }
    this._listening = false;
    queueMicrotask(() => this.emit("close"));
    return this;
  }

  address() { return this._address; }
  ref() { return this; }
  unref() { return this; }
  get listening() { return this._listening; }
}

function createServer(opts, handler) {
  return new Server(opts, handler);
}

function createConnection(...args) {
  const sock = new Socket({});
  sock.connect(...args);
  return sock;
}

function isIP(input) {
  const s = String(input);
  if (/^(\d{1,3}\.){3}\d{1,3}$/.test(s)) return 4;
  if (s.includes(":")) return 6;
  return 0;
}
function isIPv4(input) { return isIP(input) === 4; }
function isIPv6(input) { return isIP(input) === 6; }

const net = {
  Socket,
  Server,
  createServer,
  createConnection,
  connect: createConnection,
  isIP,
  isIPv4,
  isIPv6,
};
net.default = net;

module.exports = net;
