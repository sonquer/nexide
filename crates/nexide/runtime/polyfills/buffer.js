// `Buffer` polyfill - Node-compatible subclass of Uint8Array.

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
      if (value instanceof ArrayBuffer || (typeof SharedArrayBuffer !== "undefined" && value instanceof SharedArrayBuffer)) {
        const off = encodingOrOffset === undefined ? 0 : encodingOrOffset >>> 0;
        const len = length === undefined ? value.byteLength - off : length >>> 0;
        return new Buffer(value, off, len);
      }
      if (ArrayBuffer.isView(value)) {
        const src = new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
        return Buffer.__wrap(new Uint8Array(src));
      }
      if (Array.isArray(value)) {
        return Buffer.__wrap(Uint8Array.from(value));
      }
      if (value && typeof value === "object" && Array.isArray(value.data)) {
        return Buffer.__wrap(Uint8Array.from(value.data));
      }
      if (value && typeof value[Symbol.iterator] === "function") {
        return Buffer.__wrap(Uint8Array.from(value));
      }
      throw new TypeError("Buffer.from: unsupported source");
    }

    static alloc(size, fill, encoding) {
      const buf = new Buffer(size);
      if (fill !== undefined && fill !== 0) {
        buf.fill(fill, 0, size, encoding);
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
      if (typeof SharedArrayBuffer !== "undefined" && value instanceof SharedArrayBuffer) {
        return value.byteLength;
      }
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
        if (offset >= total) break;
        const take = Math.min(b.length, total - offset);
        out.set(b.subarray(0, take), offset);
        offset += take;
      }
      if (offset < total) out.fill(0, offset, total);
      return out;
    }

    static isBuffer(obj) {
      return obj instanceof Buffer;
    }

    static compare(a, b) {
      if (!(a instanceof Uint8Array) || !(b instanceof Uint8Array)) {
        throw new TypeError("Buffer.compare: arguments must be Uint8Array/Buffer");
      }
      const len = Math.min(a.length, b.length);
      for (let i = 0; i < len; i++) {
        if (a[i] !== b[i]) return a[i] < b[i] ? -1 : 1;
      }
      if (a.length === b.length) return 0;
      return a.length < b.length ? -1 : 1;
    }

    static copyBytesFrom(view, offset, length) {
      if (!ArrayBuffer.isView(view)) {
        throw new TypeError("Buffer.copyBytesFrom: view must be a TypedArray");
      }
      const elementSize = view.BYTES_PER_ELEMENT || 1;
      const o = offset === undefined ? 0 : offset | 0;
      const l = length === undefined ? view.length - o : length | 0;
      const start = view.byteOffset + o * elementSize;
      const byteLen = l * elementSize;
      const src = new Uint8Array(view.buffer, start, byteLen);
      return Buffer.__wrap(new Uint8Array(src));
    }

    static __wrap(uint8) {
      return new Buffer(uint8.buffer, uint8.byteOffset, uint8.byteLength);
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

    fill(value, offset, end, encoding) {
      if (typeof offset === "string") {
        encoding = offset;
        offset = 0;
        end = this.length;
      } else if (typeof end === "string") {
        encoding = end;
        end = this.length;
      }
      const o = offset === undefined ? 0 : offset | 0;
      const e = end === undefined ? this.length : end | 0;
      if (e <= o) return this;
      if (typeof value === "string") {
        const bytes = encode(value, encoding);
        if (bytes.length === 0) return this;
        for (let i = o; i < e; i++) this[i] = bytes[(i - o) % bytes.length];
        return this;
      }
      if (value instanceof Uint8Array) {
        if (value.length === 0) return this;
        for (let i = o; i < e; i++) this[i] = value[(i - o) % value.length];
        return this;
      }
      super.fill(value & 0xff, o, e);
      return this;
    }

    copy(target, targetStart, sourceStart, sourceEnd) {
      const ts = targetStart === undefined ? 0 : targetStart | 0;
      const ss = sourceStart === undefined ? 0 : sourceStart | 0;
      const se = sourceEnd === undefined ? this.length : sourceEnd | 0;
      if (ts >= target.length || ss >= se) return 0;
      const tAvail = target.length - ts;
      const sAvail = se - ss;
      const n = Math.min(tAvail, sAvail);
      if (target.buffer === this.buffer && Math.abs(ts - ss) < n) {
        const tmp = new Uint8Array(this.buffer, this.byteOffset + ss, n).slice();
        new Uint8Array(target.buffer, target.byteOffset + ts, n).set(tmp);
      } else {
        new Uint8Array(target.buffer, target.byteOffset + ts, n).set(
          new Uint8Array(this.buffer, this.byteOffset + ss, n),
        );
      }
      return n;
    }

    compare(target, targetStart, targetEnd, sourceStart, sourceEnd) {
      const ts = targetStart === undefined ? 0 : targetStart | 0;
      const te = targetEnd === undefined ? target.length : targetEnd | 0;
      const ss = sourceStart === undefined ? 0 : sourceStart | 0;
      const se = sourceEnd === undefined ? this.length : sourceEnd | 0;
      const a = this.subarray(ss, se);
      const b = target.subarray(ts, te);
      const len = Math.min(a.length, b.length);
      for (let i = 0; i < len; i++) {
        if (a[i] !== b[i]) return a[i] < b[i] ? -1 : 1;
      }
      if (a.length === b.length) return 0;
      return a.length < b.length ? -1 : 1;
    }

    indexOf(value, byteOffset, encoding) {
      let start;
      if (typeof byteOffset === "string") {
        encoding = byteOffset;
        start = 0;
      } else {
        start = byteOffset === undefined ? 0 : byteOffset | 0;
        if (start < 0) start = Math.max(0, this.length + start);
      }
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

    lastIndexOf(value, byteOffset, encoding) {
      let start;
      if (typeof byteOffset === "string") {
        encoding = byteOffset;
        start = this.length - 1;
      } else {
        start = byteOffset === undefined ? this.length - 1 : byteOffset | 0;
        if (start < 0) start = this.length + start;
      }
      const needle = typeof value === "number"
        ? new Uint8Array([value & 0xff])
        : (typeof value === "string" ? encode(value, encoding) : new Uint8Array(value));
      if (needle.length === 0) return Math.min(start, this.length);
      const last = Math.min(start, this.length - needle.length);
      outer: for (let i = last; i >= 0; i--) {
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

    swap16() {
      if ((this.length & 0x1) !== 0) {
        throw new RangeError("Buffer size must be a multiple of 16-bits");
      }
      for (let i = 0; i < this.length; i += 2) {
        const a = this[i];
        this[i] = this[i + 1];
        this[i + 1] = a;
      }
      return this;
    }

    swap32() {
      if ((this.length & 0x3) !== 0) {
        throw new RangeError("Buffer size must be a multiple of 32-bits");
      }
      for (let i = 0; i < this.length; i += 4) {
        const a = this[i], b = this[i + 1];
        this[i] = this[i + 3];
        this[i + 1] = this[i + 2];
        this[i + 2] = b;
        this[i + 3] = a;
      }
      return this;
    }

    swap64() {
      if ((this.length & 0x7) !== 0) {
        throw new RangeError("Buffer size must be a multiple of 64-bits");
      }
      for (let i = 0; i < this.length; i += 8) {
        const a = this[i], b = this[i + 1], c = this[i + 2], d = this[i + 3];
        this[i] = this[i + 7];
        this[i + 1] = this[i + 6];
        this[i + 2] = this[i + 5];
        this[i + 3] = this[i + 4];
        this[i + 4] = d;
        this[i + 5] = c;
        this[i + 6] = b;
        this[i + 7] = a;
      }
      return this;
    }

    toJSON() {
      const data = new Array(this.length);
      for (let i = 0; i < this.length; i++) data[i] = this[i];
      return { type: "Buffer", data };
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

    readInt8(off) {
      const v = this[off];
      return v & 0x80 ? v - 0x100 : v;
    }
    writeInt8(v, off) {
      this[off] = v & 0xff;
      return off + 1;
    }
    readInt16LE(off) {
      const v = this[off] | (this[off + 1] << 8);
      return v & 0x8000 ? v - 0x10000 : v;
    }
    readInt16BE(off) {
      const v = (this[off] << 8) | this[off + 1];
      return v & 0x8000 ? v - 0x10000 : v;
    }
    writeInt16LE(v, off) {
      this[off] = v & 0xff;
      this[off + 1] = (v >> 8) & 0xff;
      return off + 2;
    }
    writeInt16BE(v, off) {
      this[off] = (v >> 8) & 0xff;
      this[off + 1] = v & 0xff;
      return off + 2;
    }
    readInt32LE(off) {
      return ((this[off]) | (this[off + 1] << 8) | (this[off + 2] << 16) | (this[off + 3] << 24)) | 0;
    }
    readInt32BE(off) {
      return ((this[off] << 24) | (this[off + 1] << 16) | (this[off + 2] << 8) | this[off + 3]) | 0;
    }
    writeInt32LE(v, off) {
      this[off] = v & 0xff;
      this[off + 1] = (v >>> 8) & 0xff;
      this[off + 2] = (v >>> 16) & 0xff;
      this[off + 3] = (v >>> 24) & 0xff;
      return off + 4;
    }
    writeInt32BE(v, off) {
      this[off] = (v >>> 24) & 0xff;
      this[off + 1] = (v >>> 16) & 0xff;
      this[off + 2] = (v >>> 8) & 0xff;
      this[off + 3] = v & 0xff;
      return off + 4;
    }

    readUIntLE(off, byteLength) {
      let value = 0;
      let mul = 1;
      for (let i = 0; i < byteLength; i++) {
        value += this[off + i] * mul;
        mul *= 0x100;
      }
      return value;
    }
    readUIntBE(off, byteLength) {
      let value = 0;
      for (let i = 0; i < byteLength; i++) {
        value = value * 0x100 + this[off + i];
      }
      return value;
    }
    readIntLE(off, byteLength) {
      let value = this.readUIntLE(off, byteLength);
      const sign = 2 ** (8 * byteLength - 1);
      if (value >= sign) value -= sign * 2;
      return value;
    }
    readIntBE(off, byteLength) {
      let value = this.readUIntBE(off, byteLength);
      const sign = 2 ** (8 * byteLength - 1);
      if (value >= sign) value -= sign * 2;
      return value;
    }
    writeUIntLE(v, off, byteLength) {
      let value = Number(v);
      for (let i = 0; i < byteLength; i++) {
        this[off + i] = value & 0xff;
        value = Math.floor(value / 0x100);
      }
      return off + byteLength;
    }
    writeUIntBE(v, off, byteLength) {
      let value = Number(v);
      for (let i = byteLength - 1; i >= 0; i--) {
        this[off + i] = value & 0xff;
        value = Math.floor(value / 0x100);
      }
      return off + byteLength;
    }
    writeIntLE(v, off, byteLength) {
      let value = Number(v);
      if (value < 0) value += 2 ** (8 * byteLength);
      return this.writeUIntLE(value, off, byteLength);
    }
    writeIntBE(v, off, byteLength) {
      let value = Number(v);
      if (value < 0) value += 2 ** (8 * byteLength);
      return this.writeUIntBE(value, off, byteLength);
    }

    readFloatLE(off) {
      return new DataView(this.buffer, this.byteOffset, this.byteLength).getFloat32(off, true);
    }
    readFloatBE(off) {
      return new DataView(this.buffer, this.byteOffset, this.byteLength).getFloat32(off, false);
    }
    writeFloatLE(v, off) {
      new DataView(this.buffer, this.byteOffset, this.byteLength).setFloat32(off, v, true);
      return off + 4;
    }
    writeFloatBE(v, off) {
      new DataView(this.buffer, this.byteOffset, this.byteLength).setFloat32(off, v, false);
      return off + 4;
    }
    readDoubleLE(off) {
      return new DataView(this.buffer, this.byteOffset, this.byteLength).getFloat64(off, true);
    }
    readDoubleBE(off) {
      return new DataView(this.buffer, this.byteOffset, this.byteLength).getFloat64(off, false);
    }
    writeDoubleLE(v, off) {
      new DataView(this.buffer, this.byteOffset, this.byteLength).setFloat64(off, v, true);
      return off + 8;
    }
    writeDoubleBE(v, off) {
      new DataView(this.buffer, this.byteOffset, this.byteLength).setFloat64(off, v, false);
      return off + 8;
    }
    readBigInt64LE(off) {
      return new DataView(this.buffer, this.byteOffset, this.byteLength).getBigInt64(off, true);
    }
    readBigInt64BE(off) {
      return new DataView(this.buffer, this.byteOffset, this.byteLength).getBigInt64(off, false);
    }
    readBigUInt64LE(off) {
      return new DataView(this.buffer, this.byteOffset, this.byteLength).getBigUint64(off, true);
    }
    readBigUInt64BE(off) {
      return new DataView(this.buffer, this.byteOffset, this.byteLength).getBigUint64(off, false);
    }
    writeBigInt64LE(v, off) {
      new DataView(this.buffer, this.byteOffset, this.byteLength).setBigInt64(off, BigInt(v), true);
      return off + 8;
    }
    writeBigInt64BE(v, off) {
      new DataView(this.buffer, this.byteOffset, this.byteLength).setBigInt64(off, BigInt(v), false);
      return off + 8;
    }
    writeBigUInt64LE(v, off) {
      new DataView(this.buffer, this.byteOffset, this.byteLength).setBigUint64(off, BigInt(v), true);
      return off + 8;
    }
    writeBigUInt64BE(v, off) {
      new DataView(this.buffer, this.byteOffset, this.byteLength).setBigUint64(off, BigInt(v), false);
      return off + 8;
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

  Buffer.poolSize = 8192;
  Buffer.kMaxLength = 0x7fffffff;
  Buffer.INSPECT_MAX_BYTES = 50;

  for (const name of ["from", "alloc", "allocUnsafe", "allocUnsafeSlow", "byteLength", "concat", "isBuffer", "isEncoding", "compare", "copyBytesFrom"]) {
    const desc = Object.getOwnPropertyDescriptor(Buffer, name);
    if (desc && !desc.enumerable) {
      Object.defineProperty(Buffer, name, { ...desc, enumerable: true });
    }
  }

  globalThis.Buffer = Buffer;
})(globalThis);
