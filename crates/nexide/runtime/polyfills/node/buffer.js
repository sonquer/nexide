"use strict";

/**
 * Polyfill for Node.js `node:buffer`.
 *
 * `Buffer` itself is installed as a global by the host-side
 * `BufferPolyfill`; this module re-exports it together with the
 * standard `kMaxLength` / `constants` surface and a minimal
 * `Blob` / `File` implementation used by the small subset of
 * Next.js code paths that touch them. `atob` / `btoa` are also
 * re-exported (or shimmed via Buffer if the engine did not provide
 * them) so callers do not have to know which globals exist.
 *
 * The shims are deliberately compact: they implement just enough of
 * the spec for current consumers (size, type, slice, async readers
 * and a `ReadableStream` adapter). They do not attempt to model the
 * full WHATWG Blob behaviour (such as endings normalisation).
 */

/**
 * Minimal in-memory `Blob` implementation.
 *
 * Concatenates the input parts (strings UTF-8-encoded, `Uint8Array`
 * and any `ArrayBufferView`-like value taken byte-for-byte) into a
 * single backing `Uint8Array` and exposes the standard reader
 * methods. `slice` returns a new `Blob` over a copy of the byte
 * range so the source is not retained.
 */
class Blob {
  constructor(parts = [], options = {}) {
    const chunks = [];
    for (const p of parts) {
      if (p instanceof Uint8Array) chunks.push(p);
      else if (typeof p === "string") chunks.push(new TextEncoder().encode(p));
      else if (p && typeof p.byteLength === "number") chunks.push(new Uint8Array(p));
    }
    let total = 0; for (const c of chunks) total += c.byteLength;
    const buf = new Uint8Array(total);
    let off = 0; for (const c of chunks) { buf.set(c, off); off += c.byteLength; }
    this._bytes = buf;
    this.type = String(options.type || "");
  }
  /** Total byte length of the backing storage. */
  get size() { return this._bytes.byteLength; }
  /** Resolves with a fresh copy of the bytes as an `ArrayBuffer`. */
  arrayBuffer() { return Promise.resolve(this._bytes.buffer.slice(0)); }
  /** Resolves with the bytes decoded as UTF-8. */
  text() { return Promise.resolve(new TextDecoder().decode(this._bytes)); }
  /** Resolves with a fresh copy of the bytes as a `Uint8Array`. */
  bytes() { return Promise.resolve(new Uint8Array(this._bytes)); }
  /**
   * Returns a new `Blob` over the byte range `[start, end)`.
   *
   * The caller may override the MIME `type` of the resulting blob.
   * Bytes are copied so the slice is independent of the source.
   */
  slice(start = 0, end = this.size, type = "") {
    const b = new Blob([], { type });
    b._bytes = this._bytes.slice(start, end);
    return b;
  }
  /** Single-chunk `ReadableStream` over the entire backing buffer. */
  stream() {
    const bytes = this._bytes;
    return new ReadableStream({
      start(c) { c.enqueue(bytes); c.close(); },
    });
  }
}

/**
 * Minimal `File` implementation, a `Blob` with a name and an
 * optional `lastModified` timestamp (defaulting to `0`).
 */
class File extends Blob {
  constructor(parts, name, options = {}) {
    super(parts, options);
    this.name = String(name);
    this.lastModified = options.lastModified || 0;
  }
}

module.exports = {
  Buffer: globalThis.Buffer,
  Blob: globalThis.Blob || Blob,
  File: globalThis.File || File,
  constants: { MAX_LENGTH: 0x7fffffff, MAX_STRING_LENGTH: 0x1fffffff },
  kMaxLength: 0x7fffffff,
  atob: globalThis.atob || ((s) => Buffer.from(s, "base64").toString("latin1")),
  btoa: globalThis.btoa || ((s) => Buffer.from(s, "latin1").toString("base64")),
};
