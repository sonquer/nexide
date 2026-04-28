"use strict";

/**
 * Polyfill for Node.js `zlib`.
 *
 * Provides one-shot (Buffer-in / Buffer-out) compression and
 * decompression for the gzip, deflate and brotli algorithms by
 * delegating to the host ops `op_zlib_encode` / `op_zlib_decode`,
 * plus streaming Transform classes (`Deflate`, `Inflate`, `Gzip`,
 * `Gunzip`, `DeflateRaw`, `InflateRaw`) backed by the incremental
 * `op_zlib_*` slot ops. Brotli is exposed through the one-shot
 * helpers but its streaming variant rejects with
 * `ERR_NOT_AVAILABLE`; the host runtime ships only the gzip/deflate
 * streaming family.
 */

const { Transform } = require("node:stream");
const ops = Nexide.core.ops;
const Buffer = globalThis.Buffer;

function asBuf(v) {
  if (v instanceof Uint8Array) return v;
  if (typeof v === "string") return Buffer.from(v);
  throw new TypeError("zlib expects Buffer/Uint8Array/string input");
}

function syncEncode(algo, input) {
  return Buffer.from(ops.op_zlib_encode(algo, asBuf(input)));
}

function syncDecode(algo, input) {
  return Buffer.from(ops.op_zlib_decode(algo, asBuf(input)));
}

function asyncWrap(fn) {
  return function (input, opts, cb) {
    if (typeof opts === "function") { cb = opts; opts = undefined; }
    queueMicrotask(() => {
      try { cb(null, fn(input)); }
      catch (err) { cb(err); }
    });
  };
}

class ZlibTransform extends Transform {
  constructor(kind, options) {
    super(options);
    const level = options && typeof options.level === "number" ? options.level : 6;
    this._kind = kind;
    this._id = ops.op_zlib_create(kind, level);
    this._closed = false;
  }

  _transform(chunk, encoding, callback) {
    if (this._closed) {
      callback(new Error("zlib stream is closed"));
      return;
    }
    try {
      const buf = chunk instanceof Uint8Array
        ? chunk
        : Buffer.from(String(chunk), encoding || "utf8");
      const out = ops.op_zlib_feed(this._id, buf);
      if (out && out.byteLength) {
        this.push(Buffer.from(out));
      }
      callback();
    } catch (err) {
      callback(err);
    }
  }

  _flush(callback) {
    if (this._closed) {
      callback();
      return;
    }
    try {
      const out = ops.op_zlib_finish(this._id);
      this._closed = true;
      ops.op_zlib_close(this._id);
      if (out && out.byteLength) {
        this.push(Buffer.from(out));
      }
      callback();
    } catch (err) {
      callback(err);
    }
  }

  _destroy(err, callback) {
    if (!this._closed) {
      this._closed = true;
      try { ops.op_zlib_close(this._id); } catch { }
    }
    callback(err);
  }
}

class Deflate extends ZlibTransform {
  constructor(options) { super("deflate", options); }
}
class Inflate extends ZlibTransform {
  constructor(options) { super("inflate", options); }
}
class DeflateRaw extends ZlibTransform {
  constructor(options) { super("deflate-raw", options); }
}
class InflateRaw extends ZlibTransform {
  constructor(options) { super("inflate-raw", options); }
}
class Gzip extends ZlibTransform {
  constructor(options) { super("gzip", options); }
}
class Gunzip extends ZlibTransform {
  constructor(options) { super("gunzip", options); }
}

function brotliStreamingUnavailable() {
  const err = new Error(
    "Brotli streaming is not available in nexide; use brotliCompress/brotliDecompress",
  );
  err.code = "ERR_NOT_AVAILABLE";
  throw err;
}

module.exports = {
  gzipSync: (i) => syncEncode("gzip", i),
  gunzipSync: (i) => syncDecode("gzip", i),
  deflateSync: (i) => syncEncode("deflate", i),
  inflateSync: (i) => syncDecode("deflate", i),
  brotliCompressSync: (i) => syncEncode("brotli", i),
  brotliDecompressSync: (i) => syncDecode("brotli", i),

  gzip: asyncWrap((i) => syncEncode("gzip", i)),
  gunzip: asyncWrap((i) => syncDecode("gzip", i)),
  deflate: asyncWrap((i) => syncEncode("deflate", i)),
  inflate: asyncWrap((i) => syncDecode("deflate", i)),
  brotliCompress: asyncWrap((i) => syncEncode("brotli", i)),
  brotliDecompress: asyncWrap((i) => syncDecode("brotli", i)),

  createDeflate: (opts) => new Deflate(opts),
  createInflate: (opts) => new Inflate(opts),
  createDeflateRaw: (opts) => new DeflateRaw(opts),
  createInflateRaw: (opts) => new InflateRaw(opts),
  createGzip: (opts) => new Gzip(opts),
  createGunzip: (opts) => new Gunzip(opts),
  createBrotliCompress: brotliStreamingUnavailable,
  createBrotliDecompress: brotliStreamingUnavailable,

  Deflate,
  Inflate,
  DeflateRaw,
  InflateRaw,
  Gzip,
  Gunzip,

  constants: {
    Z_OK: 0, Z_STREAM_END: 1, Z_NO_FLUSH: 0, Z_FINISH: 4,
    BROTLI_OPERATION_PROCESS: 0, BROTLI_OPERATION_FINISH: 2,
  },
};
