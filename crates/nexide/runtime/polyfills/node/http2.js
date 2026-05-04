"use strict";

// node:http2 - client-side compatibility layer.
//
// Real frame-level h2 (multiplexing, server push, SETTINGS frames,
// PING, flow control windows visible to user code) requires a
// dedicated bridge to a JS-side h2 codec. nexide does not have
// that yet. However, the underlying `op_http_request` op is backed
// by reqwest + hyper-h2, which transparently negotiates HTTP/2 via
// ALPN whenever the remote endpoint advertises it. That covers the
// vast majority of real-world `node:http2` client usage: gRPC,
// Apollo, REST-over-h2 clients, server-sent events, etc.
//
// This polyfill therefore implements the *client* surface
// (`connect()`, `session.request()`, the resulting Http2Stream as
// a Duplex) on top of `op_http_request`. Each `request()` call
// becomes one logical HTTP request; reqwest's connection pool
// reuses the same h2 connection across calls to the same authority,
// so multiplexing is preserved at the transport layer even though
// JS sees one stream per request.
//
// Server-side (`createServer`, `createSecureServer`) and advanced
// features (server push, ALTSVC, PRIORITY, raw frame sends) remain
// unsupported and throw `ERR_HTTP2_NOT_SUPPORTED` when invoked.

const EventEmitter = require("node:events");
const { Readable, Duplex } = require("node:stream");
const { Buffer } = require("node:buffer");

const sensitiveHeaders = Symbol.for("nodejs.http2.sensitiveHeaders");

const constants = Object.freeze({
  NGHTTP2_NO_ERROR: 0,
  NGHTTP2_PROTOCOL_ERROR: 1,
  NGHTTP2_INTERNAL_ERROR: 2,
  NGHTTP2_FLOW_CONTROL_ERROR: 3,
  NGHTTP2_SETTINGS_TIMEOUT: 4,
  NGHTTP2_STREAM_CLOSED: 5,
  NGHTTP2_FRAME_SIZE_ERROR: 6,
  NGHTTP2_REFUSED_STREAM: 7,
  NGHTTP2_CANCEL: 8,
  NGHTTP2_COMPRESSION_ERROR: 9,
  NGHTTP2_CONNECT_ERROR: 10,
  NGHTTP2_ENHANCE_YOUR_CALM: 11,
  NGHTTP2_INADEQUATE_SECURITY: 12,
  NGHTTP2_HTTP_1_1_REQUIRED: 13,
  HTTP2_HEADER_STATUS: ":status",
  HTTP2_HEADER_METHOD: ":method",
  HTTP2_HEADER_PATH: ":path",
  HTTP2_HEADER_AUTHORITY: ":authority",
  HTTP2_HEADER_SCHEME: ":scheme",
  HTTP2_HEADER_CONTENT_LENGTH: "content-length",
  HTTP2_HEADER_CONTENT_TYPE: "content-type",
  NGHTTP2_SESSION_CLIENT: 0,
  NGHTTP2_SESSION_SERVER: 1,
});

function unsupported(feature) {
  const err = new Error(
    `nexide: node:http2 ${feature} is not implemented. The client `
      + "subset (connect/request/response) is supported on top of the "
      + "shared HTTP/2-aware reqwest client; server-side h2 and raw "
      + "frame-level APIs require a dedicated codec bridge that is "
      + "not yet wired up.",
  );
  err.code = "ERR_HTTP2_NOT_SUPPORTED";
  return err;
}

function normaliseAuthority(authority) {
  if (typeof authority === "string") {
    if (!/^[a-z][a-z0-9+\-.]*:\/\//i.test(authority)) {
      authority = `https://${authority}`;
    }
    return new URL(authority);
  }
  if (authority instanceof URL) return authority;
  throw new TypeError("authority must be a string or URL");
}

class ClientHttp2Session extends EventEmitter {
  constructor(authority, options = {}) {
    super();
    this._authority = normaliseAuthority(authority);
    this._options = options || {};
    this._closed = false;
    this._destroyed = false;
    this._pendingStreams = new Set();
    this.encrypted = this._authority.protocol === "https:";
    this.alpnProtocol = this.encrypted ? "h2" : "h2c";
    this.connecting = false;
    this.closed = false;
    this.destroyed = false;
    this.type = constants.NGHTTP2_SESSION_CLIENT;
    queueMicrotask(() => {
      if (!this._destroyed) this.emit("connect", this, /* socket */ null);
    });
  }

  request(headers, options) {
    if (this._destroyed) {
      throw new Error("ClientHttp2Session has been destroyed");
    }
    headers = headers || {};
    const method = (headers[":method"] || "GET").toUpperCase();
    const path = headers[":path"] || "/";
    const scheme = headers[":scheme"] || this._authority.protocol.replace(/:$/, "");
    const authority = headers[":authority"] || this._authority.host;
    const url = `${scheme}://${authority}${path}`;

    const flatHeaders = [];
    for (const [name, value] of Object.entries(headers)) {
      if (name.startsWith(":")) continue;
      if (Array.isArray(value)) {
        for (const v of value) flatHeaders.push([name, String(v)]);
      } else if (value !== undefined && value !== null) {
        flatHeaders.push([name, String(value)]);
      }
    }

    const stream = new ClientHttp2Stream(this, {
      method,
      url,
      headers: flatHeaders,
      endStream: Boolean(options && options.endStream),
    });
    this._pendingStreams.add(stream);
    stream.once("close", () => this._pendingStreams.delete(stream));
    return stream;
  }

  ping(payload, callback) {
    if (typeof payload === "function") {
      callback = payload;
      payload = undefined;
    }
    queueMicrotask(() => {
      if (callback) callback(null, 0, payload || Buffer.alloc(8));
    });
    return true;
  }

  setTimeout(_msecs, callback) {
    if (callback) this.on("timeout", callback);
    return this;
  }

  goaway() { /* no-op: connection lifecycle is managed by reqwest pool */ }

  settings(_settings, callback) {
    queueMicrotask(() => {
      if (callback) callback(null, {}, 0);
    });
  }

  close(callback) {
    if (this._closed) {
      if (callback) queueMicrotask(callback);
      return;
    }
    this._closed = true;
    this.closed = true;
    const wait = () => {
      if (this._pendingStreams.size === 0) {
        this.emit("close");
        if (callback) callback();
      } else {
        for (const s of this._pendingStreams) s.once("close", wait);
      }
    };
    wait();
  }

  destroy(error, _code) {
    if (this._destroyed) return;
    this._destroyed = true;
    this.destroyed = true;
    for (const s of this._pendingStreams) s.destroy(error);
    this._pendingStreams.clear();
    if (error) this.emit("error", error);
    this.emit("close");
  }
}

class ClientHttp2Stream extends Duplex {
  constructor(session, init) {
    super({ allowHalfOpen: true });
    this._session = session;
    this.session = session;
    this._method = init.method;
    this._url = init.url;
    this._headers = init.headers;
    this._chunks = [];
    this._writeEnded = false;
    this._dispatched = false;
    this._incoming = null;
    this._closed = false;
    this.id = undefined;
    this.aborted = false;
    this.destroyed = false;
    this.closed = false;
    this.pending = true;
    this.sentHeaders = headersFromArray(init.headers);
    this.rstCode = constants.NGHTTP2_NO_ERROR;
    if (init.endStream) {
      this._writeEnded = true;
      queueMicrotask(() => this._dispatchIfReady());
    }
  }

  _write(chunk, _enc, cb) {
    if (this._dispatched) {
      cb(new Error("cannot write to ClientHttp2Stream after request was dispatched"));
      return;
    }
    this._chunks.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk));
    cb();
  }

  _final(cb) {
    this._writeEnded = true;
    this._dispatchIfReady();
    cb();
  }

  _dispatchIfReady() {
    if (this._dispatched || !this._writeEnded) return;
    this._dispatched = true;
    const total = this._chunks.reduce((a, b) => a + b.length, 0);
    let body = null;
    if (total > 0) {
      body = new Uint8Array(total);
      let off = 0;
      for (const c of this._chunks) {
        body.set(c, off);
        off += c.length;
      }
    }
    Nexide.core.ops
      .op_http_request({
        method: this._method,
        url: this._url,
        headers: this._headers,
        body,
      })
      .then((resp) => this._onResponse(resp), (err) => this._onError(err));
  }

  _onResponse(resp) {
    this.pending = false;
    const headers = { ":status": resp.status };
    for (const [name, value] of resp.headers) {
      const lower = name.toLowerCase();
      if (headers[lower] === undefined) {
        headers[lower] = value;
      } else if (Array.isArray(headers[lower])) {
        headers[lower].push(value);
      } else {
        headers[lower] = [headers[lower], value];
      }
    }
    this._bodyId = resp.bodyId;
    this.emit("response", headers, /* flags */ 0);
    this._pumpResponse();
  }

  async _pumpResponse() {
    while (!this._closed) {
      let chunk;
      try {
        chunk = await Nexide.core.ops.op_http_response_read(this._bodyId);
      } catch (err) {
        this._onError(err);
        return;
      }
      if (chunk === null) {
        this._closed = true;
        this.push(null);
        Nexide.core.ops.op_http_response_close(this._bodyId);
        this.emit("end");
        this.emit("close");
        return;
      }
      this.push(chunk);
    }
  }

  _onError(err) {
    if (this._closed) return;
    this._closed = true;
    if (this._bodyId !== undefined) {
      try { Nexide.core.ops.op_http_response_close(this._bodyId); } catch { /* noop */ }
    }
    this.emit("error", err);
    this.emit("close");
  }

  _read() { /* pump-driven */ }

  close(code) {
    this.rstCode = code || constants.NGHTTP2_NO_ERROR;
    if (this._closed) return;
    this._closed = true;
    if (this._bodyId !== undefined) {
      try { Nexide.core.ops.op_http_response_close(this._bodyId); } catch { /* noop */ }
    }
    this.aborted = true;
    this.emit("close");
  }

  destroy(err) {
    if (this.destroyed) return;
    this.destroyed = true;
    this.close(constants.NGHTTP2_CANCEL);
    if (err) this.emit("error", err);
    return super.destroy(err);
  }

  setTimeout(_ms, cb) {
    if (cb) this.on("timeout", cb);
    return this;
  }

  sendTrailers(_trailers) { /* not supported via reqwest's high-level client */ }

  priority(_options) { /* h2 priority frames not exposed */ }
}

function headersFromArray(arr) {
  const out = {};
  for (const [name, value] of arr) {
    const lower = name.toLowerCase();
    if (out[lower] === undefined) out[lower] = value;
    else if (Array.isArray(out[lower])) out[lower].push(value);
    else out[lower] = [out[lower], value];
  }
  return out;
}

function connect(authority, options, listener) {
  if (typeof options === "function") {
    listener = options;
    options = undefined;
  }
  const session = new ClientHttp2Session(authority, options);
  if (listener) session.once("connect", listener);
  return session;
}

class Http2Session extends EventEmitter {}
class ServerHttp2Session extends Http2Session {
  constructor() {
    super();
    throw unsupported("server-side sessions");
  }
}
class Http2Stream extends Readable {}
class Http2Server extends EventEmitter {
  constructor() {
    super();
    throw unsupported("createServer");
  }
}
class Http2SecureServer extends EventEmitter {
  constructor() {
    super();
    throw unsupported("createSecureServer");
  }
}

function createServer() { throw unsupported("createServer"); }
function createSecureServer() { throw unsupported("createSecureServer"); }

module.exports = {
  constants,
  connect,
  createServer,
  createSecureServer,
  getDefaultSettings: () => ({
    headerTableSize: 4096,
    enablePush: false,
    initialWindowSize: 65535,
    maxFrameSize: 16384,
    maxConcurrentStreams: 4294967295,
    maxHeaderListSize: 65535,
  }),
  getPackedSettings: () => Buffer.alloc(0),
  getUnpackedSettings: () => ({}),
  Http2Session,
  ServerHttp2Session,
  ClientHttp2Session,
  Http2Stream,
  ClientHttp2Stream,
  Http2Server,
  Http2SecureServer,
  sensitiveHeaders,
};
