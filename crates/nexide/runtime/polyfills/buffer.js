// `Buffer` polyfill - Node-compatible subclass of Uint8Array.
//
// Idempotent. Implemented in pure JS; uses native TextEncoder/Decoder
// when available, with a small fallback for base64/hex. Only the
// surface required by Next.js standalone + common middleware is
// covered: from/alloc/concat/byteLength/toString/write/equals/compare/
// indexOf/readUInt*/writeUInt* in both endianesses.

((globalThis) => {
  "use strict";

  if (globalThis.Buffer && globalThis.Buffer.__nexideBuffer) {
    return;
  }

  const hasTextEncoder = typeof TextEncoder !== "undefined";
  const hasTextDecoder = typeof TextDecoder !== "undefined";
  const enc = hasTextEncoder ? new TextEncoder() : null;
  const dec = hasTextDecoder ? new TextDecoder("utf-8") : null;
  const latinDec = hasTextDecoder ? new TextDecoder("latin1") : null;

  const B64_ALPHABET =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  const B64_LOOKUP = new Int8Array(256);
  B64_LOOKUP.fill(-1);
  for (let i = 0; i < B64_ALPHABET.length; i++) {
    B64_LOOKUP[B64_ALPHABET.charCodeAt(i)] = i;
  }

  function utf8Encode(str) {
    if (enc) return enc.encode(str);
    const out = [];
    for (let i = 0; i < str.length; i++) {
      let code = str.charCodeAt(i);
      if (code >= 0xd800 && code <= 0xdbff && i + 1 < str.length) {
        const low = str.charCodeAt(i + 1);
        if (low >= 0xdc00 && low <= 0xdfff) {
          code = 0x10000 + ((code - 0xd800) << 10) + (low - 0xdc00);
          i++;
        }
      }
      if (code < 0x80) {
        out.push(code);
      } else if (code < 0x800) {
        out.push(0xc0 | (code >> 6), 0x80 | (code & 0x3f));
      } else if (code < 0x10000) {
        out.push(
          0xe0 | (code >> 12),
          0x80 | ((code >> 6) & 0x3f),
          0x80 | (code & 0x3f),
        );
      } else {
        out.push(
          0xf0 | (code >> 18),
          0x80 | ((code >> 12) & 0x3f),
          0x80 | ((code >> 6) & 0x3f),
          0x80 | (code & 0x3f),
        );
      }
    }
    return Uint8Array.from(out);
  }

  function utf8Decode(bytes) {
    if (dec) return dec.decode(bytes);
    let out = "";
    for (let i = 0; i < bytes.length; i++) out += String.fromCharCode(bytes[i]);
    return out;
  }

  function latin1Decode(bytes) {
    if (latinDec) return latinDec.decode(bytes);
    let out = "";
    for (let i = 0; i < bytes.length; i++) out += String.fromCharCode(bytes[i]);
    return out;
  }

  function hexEncode(bytes) {
    let out = "";
    for (let i = 0; i < bytes.length; i++) {
      const v = bytes[i];
      out += (v < 16 ? "0" : "") + v.toString(16);
    }
    return out;
  }

  function hexDecode(str) {
    const clean = str.replace(/[^0-9a-fA-F]/g, "");
    const len = clean.length & ~1;
    const out = new Uint8Array(len / 2);
    for (let i = 0; i < len; i += 2) {
      out[i / 2] = parseInt(clean.substr(i, 2), 16);
    }
    return out;
  }

  function base64Encode(bytes) {
    let out = "";
    let i = 0;
    for (; i + 2 < bytes.length; i += 3) {
      const a = bytes[i], b = bytes[i + 1], c = bytes[i + 2];
      out += B64_ALPHABET[a >> 2];
      out += B64_ALPHABET[((a & 0x03) << 4) | (b >> 4)];
      out += B64_ALPHABET[((b & 0x0f) << 2) | (c >> 6)];
      out += B64_ALPHABET[c & 0x3f];
    }
    const rem = bytes.length - i;
    if (rem === 1) {
      const a = bytes[i];
      out += B64_ALPHABET[a >> 2];
      out += B64_ALPHABET[(a & 0x03) << 4];
      out += "==";
    } else if (rem === 2) {
      const a = bytes[i], b = bytes[i + 1];
      out += B64_ALPHABET[a >> 2];
      out += B64_ALPHABET[((a & 0x03) << 4) | (b >> 4)];
      out += B64_ALPHABET[(b & 0x0f) << 2];
      out += "=";
    }
    return out;
  }

  function base64Decode(str) {
    const clean = str.replace(/[^A-Za-z0-9+/]/g, "");
    const padded = clean.length % 4 === 0
      ? clean
      : clean + "===".slice((clean.length + 3) % 4);
    const groups = padded.length / 4;
    let outLen = groups * 3;
    if (clean.length !== padded.length) outLen -= padded.length - clean.length;
    const out = new Uint8Array(outLen);
    let oi = 0;
    for (let i = 0; i < padded.length; i += 4) {
      const a = B64_LOOKUP[padded.charCodeAt(i)] | 0;
      const b = B64_LOOKUP[padded.charCodeAt(i + 1)] | 0;
      const c = B64_LOOKUP[padded.charCodeAt(i + 2)] | 0;
      const d = B64_LOOKUP[padded.charCodeAt(i + 3)] | 0;
      if (oi < outLen) out[oi++] = (a << 2) | (b >> 4);
      if (oi < outLen) out[oi++] = ((b & 0x0f) << 4) | (c >> 2);
      if (oi < outLen) out[oi++] = ((c & 0x03) << 6) | d;
    }
    return out;
  }

  function asciiEncode(str) {
    const out = new Uint8Array(str.length);
    for (let i = 0; i < str.length; i++) out[i] = str.charCodeAt(i) & 0x7f;
    return out;
  }

  function latin1Encode(str) {
    const out = new Uint8Array(str.length);
    for (let i = 0; i < str.length; i++) out[i] = str.charCodeAt(i) & 0xff;
    return out;
  }

  function encode(str, encoding) {
    const e = (encoding || "utf8").toLowerCase();
    switch (e) {
      case "utf8":
      case "utf-8":
        return utf8Encode(str);
      case "ascii":
        return asciiEncode(str);
      case "latin1":
      case "binary":
        return latin1Encode(str);
      case "hex":
        return hexDecode(str);
      case "base64":
      case "base64url":
        return base64Decode(
          e === "base64url" ? str.replace(/-/g, "+").replace(/_/g, "/") : str,
        );
      case "ucs2":
      case "ucs-2":
      case "utf16le":
      case "utf-16le": {
        const out = new Uint8Array(str.length * 2);
        for (let i = 0; i < str.length; i++) {
          const c = str.charCodeAt(i);
          out[i * 2] = c & 0xff;
          out[i * 2 + 1] = (c >> 8) & 0xff;
        }
        return out;
      }
      default:
        throw new TypeError("Unknown encoding: " + encoding);
    }
  }

  function decode(bytes, encoding) {
    const e = (encoding || "utf8").toLowerCase();
    switch (e) {
      case "utf8":
      case "utf-8":
        return utf8Decode(bytes);
      case "ascii":
      case "latin1":
      case "binary":
        return latin1Decode(bytes);
      case "hex":
        return hexEncode(bytes);
      case "base64":
        return base64Encode(bytes);
      case "base64url":
        return base64Encode(bytes).replace(/\+/g, "-").replace(/\//g, "_")
          .replace(/=+$/, "");
      case "ucs2":
      case "ucs-2":
      case "utf16le":
      case "utf-16le": {
        let out = "";
        for (let i = 0; i + 1 < bytes.length; i += 2) {
          out += String.fromCharCode(bytes[i] | (bytes[i + 1] << 8));
        }
        return out;
      }
      default:
        throw new TypeError("Unknown encoding: " + encoding);
    }
  }

  class Buffer extends Uint8Array {
    static from(value, encodingOrOffset, length) {
      if (typeof value === "string") {
        const bytes = encode(value, encodingOrOffset);
        return Buffer.__wrap(bytes);
      }
      if (value instanceof ArrayBuffer) {
        const view = new Uint8Array(
          value,
          encodingOrOffset || 0,
          length === undefined ? undefined : length,
        );
        return Buffer.__wrap(view.slice());
      }
      if (ArrayBuffer.isView(value)) {
        const src = new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
        return Buffer.__wrap(src.slice());
      }
      if (Array.isArray(value)) {
        return Buffer.__wrap(Uint8Array.from(value));
      }
      if (value && typeof value === "object" && Array.isArray(value.data)) {
        return Buffer.__wrap(Uint8Array.from(value.data));
      }
      throw new TypeError("Buffer.from: unsupported source");
    }

    static alloc(size, fill, encoding) {
      const buf = new Buffer(size);
      if (fill !== undefined) {
        if (typeof fill === "string") {
          const bytes = encode(fill, encoding);
          for (let i = 0; i < size; i++) buf[i] = bytes[i % bytes.length];
        } else {
          buf.fill(fill);
        }
      }
      return buf;
    }

    static allocUnsafe(size) {
      return new Buffer(size);
    }

    static byteLength(value, encoding) {
      if (typeof value === "string") return encode(value, encoding).length;
      if (ArrayBuffer.isView(value)) return value.byteLength;
      if (value instanceof ArrayBuffer) return value.byteLength;
      throw new TypeError("Buffer.byteLength: unsupported source");
    }

    static concat(list, totalLength) {
      let total = totalLength;
      if (total === undefined) {
        total = 0;
        for (const b of list) total += b.length;
      }
      const out = new Buffer(total);
      let offset = 0;
      for (const b of list) {
        const take = Math.min(b.length, total - offset);
        out.set(b.subarray(0, take), offset);
        offset += take;
        if (offset >= total) break;
      }
      return out;
    }

    static isBuffer(obj) {
      return obj instanceof Buffer;
    }

    static __wrap(uint8) {
      const out = new Buffer(uint8.buffer, uint8.byteOffset, uint8.byteLength);
      return out;
    }

    toString(encoding, start, end) {
      const s = start || 0;
      const e = end === undefined ? this.length : end;
      return decode(this.subarray(s, e), encoding);
    }

    write(string, offset, length, encoding) {
      let o = 0, l, enc2;
      if (typeof offset === "string") { enc2 = offset; o = 0; l = this.length; }
      else { o = offset || 0; l = length === undefined ? this.length - o : length; enc2 = encoding; }
      const bytes = encode(string, enc2);
      const take = Math.min(bytes.length, l);
      this.set(bytes.subarray(0, take), o);
      return take;
    }

    slice(start, end) {
      return Buffer.__wrap(super.subarray(start, end));
    }

    subarray(start, end) {
      return Buffer.__wrap(super.subarray(start, end));
    }

    equals(other) {
      if (!(other instanceof Uint8Array) || this.length !== other.length) return false;
      for (let i = 0; i < this.length; i++) if (this[i] !== other[i]) return false;
      return true;
    }

    compare(other) {
      const len = Math.min(this.length, other.length);
      for (let i = 0; i < len; i++) {
        if (this[i] !== other[i]) return this[i] < other[i] ? -1 : 1;
      }
      if (this.length === other.length) return 0;
      return this.length < other.length ? -1 : 1;
    }

    indexOf(value, byteOffset, encoding) {
      const start = byteOffset | 0;
      const needle = typeof value === "number"
        ? new Uint8Array([value & 0xff])
        : (typeof value === "string" ? encode(value, encoding) : new Uint8Array(value));
      if (needle.length === 0) return start;
      outer: for (let i = start; i <= this.length - needle.length; i++) {
        for (let j = 0; j < needle.length; j++) {
          if (this[i + j] !== needle[j]) continue outer;
        }
        return i;
      }
      return -1;
    }

    includes(value, byteOffset, encoding) {
      return this.indexOf(value, byteOffset, encoding) !== -1;
    }

    readUInt8(off) { return this[off]; }
    writeUInt8(v, off) { this[off] = v & 0xff; return off + 1; }

    readUInt16LE(off) { return this[off] | (this[off + 1] << 8); }
    readUInt16BE(off) { return (this[off] << 8) | this[off + 1]; }
    writeUInt16LE(v, off) {
      this[off] = v & 0xff; this[off + 1] = (v >> 8) & 0xff; return off + 2;
    }
    writeUInt16BE(v, off) {
      this[off] = (v >> 8) & 0xff; this[off + 1] = v & 0xff; return off + 2;
    }

    readUInt32LE(off) {
      return ((this[off]) | (this[off + 1] << 8) | (this[off + 2] << 16) | (this[off + 3] << 24)) >>> 0;
    }
    readUInt32BE(off) {
      return ((this[off] << 24) | (this[off + 1] << 16) | (this[off + 2] << 8) | this[off + 3]) >>> 0;
    }
    writeUInt32LE(v, off) {
      this[off] = v & 0xff;
      this[off + 1] = (v >>> 8) & 0xff;
      this[off + 2] = (v >>> 16) & 0xff;
      this[off + 3] = (v >>> 24) & 0xff;
      return off + 4;
    }
    writeUInt32BE(v, off) {
      this[off] = (v >>> 24) & 0xff;
      this[off + 1] = (v >>> 16) & 0xff;
      this[off + 2] = (v >>> 8) & 0xff;
      this[off + 3] = v & 0xff;
      return off + 4;
    }
  }

  Object.defineProperty(Buffer, "__nexideBuffer", {
    value: true,
    enumerable: false,
    configurable: false,
    writable: false,
  });

  Buffer.allocUnsafeSlow = function allocUnsafeSlow(size) {
    return Buffer.allocUnsafe(size);
  };

  Buffer.isEncoding = function isEncoding(enc) {
    if (typeof enc !== "string") return false;
    switch (enc.toLowerCase()) {
      case "utf8":
      case "utf-8":
      case "ascii":
      case "latin1":
      case "binary":
      case "base64":
      case "base64url":
      case "hex":
      case "ucs2":
      case "ucs-2":
      case "utf16le":
      case "utf-16le":
        return true;
      default:
        return false;
    }
  };

  for (const name of ["from", "alloc", "allocUnsafe", "allocUnsafeSlow", "byteLength", "concat", "isBuffer", "isEncoding"]) {
    const desc = Object.getOwnPropertyDescriptor(Buffer, name);
    if (desc && !desc.enumerable) {
      Object.defineProperty(Buffer, name, { ...desc, enumerable: true });
    }
  }

  globalThis.Buffer = Buffer;
})(globalThis);
