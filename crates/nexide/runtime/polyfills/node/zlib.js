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
class BrotliCompress extends ZlibTransform {
  constructor(options) { super("brotli-compress", options); }
}
class BrotliDecompress extends ZlibTransform {
  constructor(options) { super("brotli-decompress", options); }
}

function unzipDecode(input) {
  const buf = asBuf(input);
  if (buf.length >= 2 && buf[0] === 0x1f && buf[1] === 0x8b) {
    return Buffer.from(ops.op_zlib_decode("gzip", buf));
  }
  return Buffer.from(ops.op_zlib_decode("deflate", buf));
}

class BrotliUnavailable {
  constructor() {
    throw new Error("BrotliUnavailable is no longer used");
  }
}
// Kept exported for ABI stability with previous pre-streaming releases.
void BrotliUnavailable;

module.exports = {
  gzipSync: (i) => syncEncode("gzip", i),
  gunzipSync: (i) => syncDecode("gzip", i),
  deflateSync: (i) => syncEncode("deflate", i),
  inflateSync: (i) => syncDecode("deflate", i),
  deflateRawSync: (i) => syncEncode("deflate-raw", i),
  inflateRawSync: (i) => syncDecode("deflate-raw", i),
  unzipSync: (i) => unzipDecode(i),
  brotliCompressSync: (i) => syncEncode("brotli", i),
  brotliDecompressSync: (i) => syncDecode("brotli", i),

  gzip: asyncWrap((i) => syncEncode("gzip", i)),
  gunzip: asyncWrap((i) => syncDecode("gzip", i)),
  deflate: asyncWrap((i) => syncEncode("deflate", i)),
  inflate: asyncWrap((i) => syncDecode("deflate", i)),
  deflateRaw: asyncWrap((i) => syncEncode("deflate-raw", i)),
  inflateRaw: asyncWrap((i) => syncDecode("deflate-raw", i)),
  unzip: asyncWrap((i) => unzipDecode(i)),
  brotliCompress: asyncWrap((i) => syncEncode("brotli", i)),
  brotliDecompress: asyncWrap((i) => syncDecode("brotli", i)),

  createDeflate: (opts) => new Deflate(opts),
  createInflate: (opts) => new Inflate(opts),
  createDeflateRaw: (opts) => new DeflateRaw(opts),
  createInflateRaw: (opts) => new InflateRaw(opts),
  createGzip: (opts) => new Gzip(opts),
  createGunzip: (opts) => new Gunzip(opts),
  createUnzip: (opts) => new Gunzip(opts),
  createBrotliCompress: (opts) => new BrotliCompress(opts),
  createBrotliDecompress: (opts) => new BrotliDecompress(opts),

  Deflate,
  Inflate,
  DeflateRaw,
  InflateRaw,
  Gzip,
  Gunzip,
  Unzip: Gunzip,
  BrotliCompress,
  BrotliDecompress,

  constants: {
    Z_NO_FLUSH: 0,
    Z_PARTIAL_FLUSH: 1,
    Z_SYNC_FLUSH: 2,
    Z_FULL_FLUSH: 3,
    Z_FINISH: 4,
    Z_BLOCK: 5,
    Z_TREES: 6,
    Z_OK: 0,
    Z_STREAM_END: 1,
    Z_NEED_DICT: 2,
    Z_ERRNO: -1,
    Z_STREAM_ERROR: -2,
    Z_DATA_ERROR: -3,
    Z_MEM_ERROR: -4,
    Z_BUF_ERROR: -5,
    Z_VERSION_ERROR: -6,
    Z_NO_COMPRESSION: 0,
    Z_BEST_SPEED: 1,
    Z_BEST_COMPRESSION: 9,
    Z_DEFAULT_COMPRESSION: -1,
    Z_FILTERED: 1,
    Z_HUFFMAN_ONLY: 2,
    Z_RLE: 3,
    Z_FIXED: 4,
    Z_DEFAULT_STRATEGY: 0,
    Z_BINARY: 0,
    Z_TEXT: 1,
    Z_UNKNOWN: 2,
    Z_DEFLATED: 8,
    Z_MIN_WINDOWBITS: 8,
    Z_MAX_WINDOWBITS: 15,
    Z_DEFAULT_WINDOWBITS: 15,
    Z_MIN_CHUNK: 64,
    Z_MAX_CHUNK: Infinity,
    Z_DEFAULT_CHUNK: 16384,
    Z_MIN_MEMLEVEL: 1,
    Z_MAX_MEMLEVEL: 9,
    Z_DEFAULT_MEMLEVEL: 8,
    Z_MIN_LEVEL: -1,
    Z_MAX_LEVEL: 9,
    Z_DEFAULT_LEVEL: -1,
    DEFLATE: 1,
    INFLATE: 2,
    GZIP: 3,
    GUNZIP: 4,
    DEFLATERAW: 5,
    INFLATERAW: 6,
    UNZIP: 7,
    BROTLI_DECODE: 8,
    BROTLI_ENCODE: 9,
    BROTLI_OPERATION_PROCESS: 0,
    BROTLI_OPERATION_FLUSH: 1,
    BROTLI_OPERATION_FINISH: 2,
    BROTLI_OPERATION_EMIT_METADATA: 3,
    BROTLI_PARAM_MODE: 0,
    BROTLI_MODE_GENERIC: 0,
    BROTLI_MODE_TEXT: 1,
    BROTLI_MODE_FONT: 2,
    BROTLI_DEFAULT_MODE: 0,
    BROTLI_PARAM_QUALITY: 1,
    BROTLI_MIN_QUALITY: 0,
    BROTLI_MAX_QUALITY: 11,
    BROTLI_DEFAULT_QUALITY: 11,
    BROTLI_PARAM_LGWIN: 2,
    BROTLI_MIN_WINDOW_BITS: 10,
    BROTLI_MAX_WINDOW_BITS: 24,
    BROTLI_LARGE_MAX_WINDOW_BITS: 30,
    BROTLI_DEFAULT_WINDOW: 22,
    BROTLI_PARAM_LGBLOCK: 3,
    BROTLI_MIN_INPUT_BLOCK_BITS: 16,
    BROTLI_MAX_INPUT_BLOCK_BITS: 24,
    BROTLI_PARAM_DISABLE_LITERAL_CONTEXT_MODELING: 4,
    BROTLI_PARAM_SIZE_HINT: 5,
    BROTLI_PARAM_LARGE_WINDOW: 6,
    BROTLI_PARAM_NPOSTFIX: 7,
    BROTLI_PARAM_NDIRECT: 8,
    BROTLI_DECODER_RESULT_ERROR: 0,
    BROTLI_DECODER_RESULT_SUCCESS: 1,
    BROTLI_DECODER_RESULT_NEEDS_MORE_INPUT: 2,
    BROTLI_DECODER_RESULT_NEEDS_MORE_OUTPUT: 3,
    BROTLI_DECODER_PARAM_DISABLE_RING_BUFFER_REALLOCATION: 0,
    BROTLI_DECODER_PARAM_LARGE_WINDOW: 1,
  },
};
