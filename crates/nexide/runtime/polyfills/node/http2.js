"use strict";

// node:http2 - nexide does not implement HTTP/2 server/client. Many
// libraries probe for it via `try { require('http2') } catch {}` and
// silently fall back to HTTP/1.1, so we expose the module surface but
// throw on actual use rather than at require-time.

function unsupported() {
  throw new Error("nexide: node:http2 is not implemented in this runtime");
}

class Http2Session { constructor() { unsupported(); } }
class ServerHttp2Session extends Http2Session {}
class ClientHttp2Session extends Http2Session {}
class Http2Stream { constructor() { unsupported(); } }
class Http2Server { constructor() { unsupported(); } }
class Http2SecureServer { constructor() { unsupported(); } }

const constants = Object.freeze({
  NGHTTP2_NO_ERROR: 0,
  NGHTTP2_PROTOCOL_ERROR: 1,
  NGHTTP2_INTERNAL_ERROR: 2,
  HTTP2_HEADER_STATUS: ":status",
  HTTP2_HEADER_METHOD: ":method",
  HTTP2_HEADER_PATH: ":path",
  HTTP2_HEADER_AUTHORITY: ":authority",
  HTTP2_HEADER_SCHEME: ":scheme",
});

module.exports = {
  constants,
  createServer: unsupported,
  createSecureServer: unsupported,
  connect: unsupported,
  getDefaultSettings: () => ({}),
  getPackedSettings: () => Buffer.alloc(0),
  getUnpackedSettings: () => ({}),
  Http2Session,
  ServerHttp2Session,
  ClientHttp2Session,
  Http2Stream,
  Http2Server,
  Http2SecureServer,
  sensitiveHeaders: Symbol("nodejs.http2.sensitiveHeaders"),
};
