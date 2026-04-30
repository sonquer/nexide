// WHATWG Fetch / Streams stubs - minimal surface required to let
// Next.js' bundled spec adapters load. This is NOT a real fetch
// implementation; it satisfies `class extends Request` and
// `instanceof Headers` checks so module-graph evaluation can proceed.

(function () {
  const _trace = (msg) => {
    if (typeof Nexide !== "undefined" && Nexide.core && Nexide.core.ops)
      Nexide.core.ops.op_nexide_log(0, "web_apis: " + msg);
  };

  if (typeof globalThis.performance === "undefined") {
    const origin = Date.now();
    const performance = {
      now() {
        const ts = (typeof Nexide !== "undefined" && Nexide.core && Nexide.core.ops && Nexide.core.ops.op_now)
          ? Nexide.core.ops.op_now()
          : null;
        if (typeof ts === "number") return ts;
        return Date.now() - origin;
      },
      timeOrigin: origin,
      mark() {},
      measure() {},
      clearMarks() {},
      clearMeasures() {},
      getEntries() { return []; },
      getEntriesByName() { return []; },
      getEntriesByType() { return []; },
    };
    Object.defineProperty(globalThis, "performance", {
      value: performance, writable: true, configurable: true, enumerable: false,
    });
  }

  if (typeof globalThis.TextEncoder === "undefined") {
    class TextEncoder {
      get encoding() { return "utf-8"; }
      encode(input = "") {
        const s = String(input);
        const bytes = [];
        for (let i = 0; i < s.length; i++) {
          let c = s.charCodeAt(i);
          if (c >= 0xD800 && c <= 0xDBFF && i + 1 < s.length) {
            const c2 = s.charCodeAt(i + 1);
            if (c2 >= 0xDC00 && c2 <= 0xDFFF) {
              c = 0x10000 + ((c - 0xD800) << 10) + (c2 - 0xDC00); i++;
            }
          }
          if (c < 0x80) bytes.push(c);
          else if (c < 0x800) bytes.push(0xC0 | (c >> 6), 0x80 | (c & 0x3F));
          else if (c < 0x10000) bytes.push(0xE0 | (c >> 12), 0x80 | ((c >> 6) & 0x3F), 0x80 | (c & 0x3F));
          else bytes.push(0xF0 | (c >> 18), 0x80 | ((c >> 12) & 0x3F), 0x80 | ((c >> 6) & 0x3F), 0x80 | (c & 0x3F));
        }
        return new Uint8Array(bytes);
      }
      encodeInto(source, dest) {
        const enc = this.encode(source);
        const n = Math.min(enc.length, dest.length);
        dest.set(enc.subarray(0, n));
        return { read: source.length, written: n };
      }
    }
    globalThis.TextEncoder = TextEncoder;
  }

  if (typeof globalThis.TextDecoder === "undefined") {
    class TextDecoder {
      constructor(label = "utf-8", options = {}) {
        this.encoding = String(label).toLowerCase();
        this.fatal = !!options.fatal;
        this.ignoreBOM = !!options.ignoreBOM;
        this._pending = null;
        this._pendingNeed = 0;
      }
      decode(input, options) {
        const stream = !!(options && options.stream);
        let u;
        if (input == null) {
          u = new Uint8Array(0);
        } else if (input instanceof Uint8Array) {
          u = input;
        } else if (input.buffer instanceof ArrayBuffer) {
          u = new Uint8Array(input.buffer, input.byteOffset || 0, input.byteLength);
        } else {
          u = new Uint8Array(input);
        }

        let bytes;
        if (this._pending && this._pending.length > 0) {
          bytes = new Uint8Array(this._pending.length + u.length);
          bytes.set(this._pending, 0);
          bytes.set(u, this._pending.length);
          this._pending = null;
          this._pendingNeed = 0;
        } else {
          bytes = u;
        }

        let out = "";
        let i = 0;
        const n = bytes.length;
        while (i < n) {
          const b = bytes[i];
          let need;
          if (b < 0x80) need = 1;
          else if ((b & 0xE0) === 0xC0) need = 2;
          else if ((b & 0xF0) === 0xE0) need = 3;
          else if ((b & 0xF8) === 0xF0) need = 4;
          else { out += String.fromCharCode(0xFFFD); i += 1; continue; }

          if (i + need > n) {
            if (stream) {
              this._pending = bytes.slice(i);
              this._pendingNeed = need - (n - i);
              i = n;
              break;
            }
            out += String.fromCharCode(0xFFFD);
            i = n;
            break;
          }

          let cp;
          if (need === 1) cp = b;
          else if (need === 2) cp = ((b & 0x1F) << 6) | (bytes[i + 1] & 0x3F);
          else if (need === 3) cp = ((b & 0x0F) << 12) | ((bytes[i + 1] & 0x3F) << 6) | (bytes[i + 2] & 0x3F);
          else cp = ((b & 0x07) << 18) | ((bytes[i + 1] & 0x3F) << 12) | ((bytes[i + 2] & 0x3F) << 6) | (bytes[i + 3] & 0x3F);

          i += need;

          if (cp > 0xFFFF) {
            cp -= 0x10000;
            out += String.fromCharCode(0xD800 + (cp >> 10), 0xDC00 + (cp & 0x3FF));
          } else {
            out += String.fromCharCode(cp);
          }
        }

        if (!stream && this._pending) {
          out += String.fromCharCode(0xFFFD);
          this._pending = null;
          this._pendingNeed = 0;
        }

        return out;
      }
    }
    globalThis.TextDecoder = TextDecoder;
  }

  if (typeof globalThis.structuredClone === "undefined") {
    globalThis.structuredClone = function structuredClone(value) {
      if (value === null || typeof value !== "object") return value;
      if (typeof value.toJSON === "function") return JSON.parse(JSON.stringify(value));
      try { return JSON.parse(JSON.stringify(value)); }
      catch (_) { return value; }
    };
    Object.defineProperty(globalThis, "structuredClone", { value: globalThis.structuredClone, writable: true, configurable: true });
  }

  if (typeof globalThis.atob === "undefined") {
    const B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    globalThis.btoa = function btoa(s) {
      let out = "";
      const str = String(s);
      for (let i = 0; i < str.length;) {
        const b1 = str.charCodeAt(i++) & 0xFF;
        const b2 = i < str.length ? str.charCodeAt(i++) & 0xFF : NaN;
        const b3 = i < str.length ? str.charCodeAt(i++) & 0xFF : NaN;
        out += B64[b1 >> 2];
        out += B64[((b1 & 3) << 4) | (isNaN(b2) ? 0 : (b2 >> 4))];
        out += isNaN(b2) ? "=" : B64[((b2 & 0xF) << 2) | (isNaN(b3) ? 0 : (b3 >> 6))];
        out += isNaN(b3) ? "=" : B64[b3 & 0x3F];
      }
      return out;
    };
    globalThis.atob = function atob(s) {
      const str = String(s).replace(/=+$/, "");
      let out = "";
      let buf = 0, bits = 0;
      for (const ch of str) {
        const idx = B64.indexOf(ch);
        if (idx < 0) continue;
        buf = (buf << 6) | idx; bits += 6;
        if (bits >= 8) { bits -= 8; out += String.fromCharCode((buf >> bits) & 0xFF); }
      }
      return out;
    };
  }

  if (typeof globalThis.Event === "undefined") {
    class Event {
      constructor(type, init = {}) {
        this.type = String(type);
        this.bubbles = !!init.bubbles;
        this.cancelable = !!init.cancelable;
        this.defaultPrevented = false;
        this.target = null;
        this.currentTarget = null;
        this.timeStamp = Date.now();
      }
      preventDefault() { if (this.cancelable) this.defaultPrevented = true; }
      stopPropagation() {}
      stopImmediatePropagation() {}
    }
    globalThis.Event = Event;
  }

  if (typeof globalThis.EventTarget === "undefined") {
    class EventTarget {
      constructor() { this.__listeners = new Map(); }
      addEventListener(type, listener, _options) {
        if (typeof listener !== "function" && (!listener || typeof listener.handleEvent !== "function")) return;
        const list = this.__listeners.get(type) || [];
        list.push(listener);
        this.__listeners.set(type, list);
      }
      removeEventListener(type, listener) {
        const list = this.__listeners.get(type);
        if (!list) return;
        const idx = list.indexOf(listener);
        if (idx >= 0) list.splice(idx, 1);
      }
      dispatchEvent(event) {
        event.target = this; event.currentTarget = this;
        const list = this.__listeners.get(event.type);
        if (!list) return true;
        for (const l of list.slice()) {
          try { typeof l === "function" ? l.call(this, event) : l.handleEvent(event); }
          catch (e) { _trace("EventTarget listener threw: " + e); }
        }
        return !event.defaultPrevented;
      }
    }
    globalThis.EventTarget = EventTarget;
  }

  if (typeof globalThis.Headers === "undefined") {
    class Headers {
      constructor(init) {
        this._map = new Map();
        if (init) {
          if (init instanceof Headers) {
            for (const [k, v] of init.entries()) this.append(k, v);
          } else if (Array.isArray(init)) {
            for (const [k, v] of init) this.append(k, v);
          } else {
            for (const k of Object.keys(init)) this.append(k, init[k]);
          }
        }
      }
      append(k, v) { const key = String(k).toLowerCase(); const cur = this._map.get(key); this._map.set(key, cur ? `${cur}, ${v}` : String(v)); }
      delete(k) { this._map.delete(String(k).toLowerCase()); }
      get(k) { return this._map.get(String(k).toLowerCase()) ?? null; }
      has(k) { return this._map.has(String(k).toLowerCase()); }
      set(k, v) { this._map.set(String(k).toLowerCase(), String(v)); }
      forEach(cb, thisArg) { for (const [k, v] of this._map) cb.call(thisArg, v, k, this); }
      *entries() { yield* this._map.entries(); }
      *keys() { yield* this._map.keys(); }
      *values() { yield* this._map.values(); }
      [Symbol.iterator]() { return this._map.entries(); }
    }
    globalThis.Headers = Headers;
  }

  if (typeof globalThis.AbortController === "undefined") {
    class AbortSignal extends EventTarget {
      constructor() { super(); this.aborted = false; this.reason = undefined; }
      throwIfAborted() { if (this.aborted) throw this.reason; }
      static abort(reason) { const s = new AbortSignal(); s.aborted = true; s.reason = reason; return s; }
      static timeout(ms) { const s = new AbortSignal(); setTimeout(() => { s.aborted = true; s.reason = new Error("TimeoutError"); s.dispatchEvent(new Event("abort")); }, ms); return s; }
    }
    class AbortController {
      constructor() { this.signal = new AbortSignal(); }
      abort(reason) { if (this.signal.aborted) return; this.signal.aborted = true; this.signal.reason = reason; this.signal.dispatchEvent(new Event("abort")); }
    }
    globalThis.AbortSignal = AbortSignal;
    globalThis.AbortController = AbortController;
  }

  function readBody(input) {
    if (input == null) return new Uint8Array(0);
    if (input instanceof Uint8Array) return input;
    if (typeof input === "string") {
      const enc = new TextEncoder();
      return enc.encode(input);
    }
    if (input.buffer instanceof ArrayBuffer) return new Uint8Array(input.buffer);
    return new Uint8Array(0);
  }

  function isNodeReadable(value) {
    return (
      value != null &&
      typeof value === "object" &&
      typeof value.on === "function" &&
      typeof value.off === "function"
    );
  }

  async function consumeStream(stream) {
    const reader = stream.getReader();
    const chunks = [];
    let total = 0;
    for (;;) {
      const { value, done } = await reader.read();
      if (done) break;
      const bytes = value instanceof Uint8Array ? value : new Uint8Array(value);
      chunks.push(bytes);
      total += bytes.byteLength;
    }
    const out = new Uint8Array(total);
    let offset = 0;
    for (const c of chunks) {
      out.set(c, offset);
      offset += c.byteLength;
    }
    return out;
  }

  function chunkToBytes(chunk) {
    if (chunk instanceof Uint8Array) return chunk;
    if (typeof chunk === "string") return new TextEncoder().encode(chunk);
    if (chunk && chunk.buffer instanceof ArrayBuffer) {
      return new Uint8Array(chunk.buffer, chunk.byteOffset || 0, chunk.byteLength);
    }
    return new Uint8Array(chunk);
  }

  function nodeReadableToStream(req) {
    return new globalThis.ReadableStream({
      start(controller) {
        if (Array.isArray(req._buffer) && req._buffer.length) {
          const drained = req._buffer.splice(0);
          for (const chunk of drained) controller.enqueue(chunkToBytes(chunk));
        }
        if (req._ended) {
          try { controller.close(); } catch { }
          return;
        }
        const onData = (chunk) => controller.enqueue(chunkToBytes(chunk));
        const onEnd = () => {
          try { controller.close(); } catch { }
        };
        const onError = (err) => {
          try { controller.error(err); } catch { }
        };
        req.on("data", onData);
        req.on("end", onEnd);
        req.on("error", onError);
      },
    });
  }

  function bodyToReadableStream(raw) {
    if (raw == null) return null;
    if (typeof globalThis.ReadableStream === "undefined") return null;
    if (raw instanceof globalThis.ReadableStream) return raw;
    if (isNodeReadable(raw)) return nodeReadableToStream(raw);
    const bytes = readBody(raw);
    return new globalThis.ReadableStream({
      start(controller) {
        if (bytes.byteLength > 0) controller.enqueue(bytes);
        controller.close();
      },
    });
  }

  async function consumeBody(raw) {
    if (raw == null) return new Uint8Array(0);
    if (raw instanceof globalThis.ReadableStream) return await consumeStream(raw);
    if (isNodeReadable(raw)) return await consumeStream(nodeReadableToStream(raw));
    return readBody(raw);
  }

  if (typeof globalThis.Request === "undefined") {
    class Request {
      constructor(input, init = {}) {
        const url = typeof input === "string" ? input : (input && input.url) || "";
        const method = (init.method || (input && input.method) || "GET").toUpperCase();
        const headers = new globalThis.Headers(init.headers || (input && input.headers) || {});
        const rawBody = init.body ?? (input && (input._rawBody !== undefined ? input._rawBody : input.body)) ?? null;
        const signal = init.signal || (input && input.signal) || new globalThis.AbortSignal();
        Object.defineProperty(this, "url", { value: url, writable: true, configurable: true, enumerable: true });
        Object.defineProperty(this, "method", { value: method, writable: true, configurable: true, enumerable: true });
        Object.defineProperty(this, "headers", { value: headers, writable: true, configurable: true, enumerable: true });
        Object.defineProperty(this, "_rawBody", { value: rawBody, writable: true, configurable: true, enumerable: false });
        Object.defineProperty(this, "signal", { value: signal, writable: true, configurable: true, enumerable: true });
        this.credentials = init.credentials || "same-origin";
        this.mode = init.mode || "cors";
        this.cache = init.cache || "default";
        this.redirect = init.redirect || "follow";
        this.referrer = init.referrer || "";
        this.bodyUsed = false;
      }
      get body() { return bodyToReadableStream(this._rawBody); }
      clone() { return new Request(this.url, this); }
      async arrayBuffer() { return (await consumeBody(this._rawBody)).buffer; }
      async text() { return new TextDecoder().decode(await consumeBody(this._rawBody)); }
      async json() { return JSON.parse(await this.text()); }
    }
    globalThis.Request = Request;
  }

  if (typeof globalThis.Response === "undefined") {
    class Response {
      constructor(body = null, init = {}) {
        this._rawBody = body;
        this.status = init.status ?? 200;
        this.statusText = init.statusText ?? "";
        this.headers = new globalThis.Headers(init.headers || {});
        this.ok = this.status >= 200 && this.status < 300;
        this.redirected = false;
        this.type = "default";
        this.url = "";
        this.bodyUsed = false;
      }
      get body() { return bodyToReadableStream(this._rawBody); }
      clone() { return new Response(this._rawBody, { status: this.status, statusText: this.statusText, headers: this.headers }); }
      static error() { const r = new Response(null, { status: 0 }); r.type = "error"; return r; }
      static redirect(url, status = 302) { return new Response(null, { status, headers: { location: url } }); }
      static json(data, init) { return new Response(JSON.stringify(data), { ...(init || {}), headers: { "content-type": "application/json", ...((init && init.headers) || {}) } }); }
      async arrayBuffer() { return (await consumeBody(this._rawBody)).buffer; }
      async text() { return new TextDecoder().decode(await consumeBody(this._rawBody)); }
      async json() { return JSON.parse(await this.text()); }
    }
    globalThis.Response = Response;
  }

  if (typeof globalThis.fetch === "undefined") {
    const httpOps = (typeof Nexide !== "undefined" && Nexide.core && Nexide.core.ops) ? Nexide.core.ops : null;

    async function collectBody(raw) {
      if (raw == null) return null;
      const bytes = await consumeBody(raw);
      return bytes.byteLength === 0 ? null : bytes;
    }

    function makeResponseStream(bodyId) {
      if (typeof globalThis.ReadableStream === "undefined") return null;
      let cancelled = false;
      return new globalThis.ReadableStream({
        async pull(controller) {
          if (cancelled) return;
          try {
            const chunk = await httpOps.op_http_response_read(bodyId);
            if (chunk === null) {
              cancelled = true;
              try { httpOps.op_http_response_close(bodyId); } catch { }
              controller.close();
              return;
            }
            controller.enqueue(chunk);
          } catch (err) {
            cancelled = true;
            try { httpOps.op_http_response_close(bodyId); } catch { }
            controller.error(err);
          }
        },
        cancel() {
          cancelled = true;
          try { httpOps.op_http_response_close(bodyId); } catch { }
        },
      });
    }

    globalThis.fetch = async function fetch(input, init = {}) {
      if (!httpOps || typeof httpOps.op_http_request !== "function") {
        throw new Error("fetch requires the http host ops; runtime is not initialised");
      }
      const url = typeof input === "string"
        ? input
        : (input && typeof input.url === "string" ? input.url : String(input));
      const method = String((init.method || (input && input.method) || "GET")).toUpperCase();
      const headersIn = init.headers
        || (input && input.headers)
        || (typeof globalThis.Headers !== "undefined" ? new globalThis.Headers() : []);
      const headers = [];
      if (headersIn && typeof headersIn.forEach === "function") {
        headersIn.forEach((value, name) => headers.push([String(name), String(value)]));
      } else if (Array.isArray(headersIn)) {
        for (const pair of headersIn) headers.push([String(pair[0]), String(pair[1])]);
      } else if (headersIn && typeof headersIn === "object") {
        for (const [name, value] of Object.entries(headersIn)) headers.push([String(name), String(value)]);
      }
      const rawBody = init.body ?? (input && input._rawBody) ?? null;
      const body = await collectBody(rawBody);

      const resp = await httpOps.op_http_request({ method, url, headers, body });

      const responseHeaders = typeof globalThis.Headers !== "undefined" ? new globalThis.Headers() : null;
      if (responseHeaders) {
        for (const [name, value] of resp.headers) responseHeaders.append(name, value);
      }
      const stream = makeResponseStream(resp.bodyId);
      const Response = globalThis.Response;
      const out = new Response(stream, {
        status: resp.status,
        statusText: resp.statusText,
        headers: responseHeaders || resp.headers,
      });
      Object.defineProperty(out, "url", { value: url, writable: true, configurable: true, enumerable: true });
      return out;
    };
  }

  if (typeof globalThis.FormData === "undefined") {
    class FormData {
      constructor() { this._entries = []; }
      append(k, v) { this._entries.push([String(k), v]); }
      delete(k) { this._entries = this._entries.filter(([n]) => n !== k); }
      get(k) { const e = this._entries.find(([n]) => n === k); return e ? e[1] : null; }
      getAll(k) { return this._entries.filter(([n]) => n === k).map(([, v]) => v); }
      has(k) { return this._entries.some(([n]) => n === k); }
      set(k, v) { this.delete(k); this.append(k, v); }
      *entries() { yield* this._entries; }
      *keys() { for (const [k] of this._entries) yield k; }
      *values() { for (const [, v] of this._entries) yield v; }
      [Symbol.iterator]() { return this.entries(); }
    }
    globalThis.FormData = FormData;
  }

  if (typeof globalThis.Blob === "undefined") {
    class Blob {
      constructor(parts = [], options = {}) {
        const enc = new TextEncoder();
        const chunks = [];
        let total = 0;
        for (const p of parts) {
          const u = p instanceof Uint8Array ? p
            : typeof p === "string" ? enc.encode(p)
            : p instanceof ArrayBuffer ? new Uint8Array(p)
            : new Uint8Array(0);
          chunks.push(u); total += u.byteLength;
        }
        const buf = new Uint8Array(total);
        let off = 0;
        for (const c of chunks) { buf.set(c, off); off += c.byteLength; }
        this._buf = buf;
        this.size = buf.byteLength;
        this.type = options.type || "";
      }
      async arrayBuffer() { return this._buf.buffer.slice(0); }
      async text() { return new TextDecoder().decode(this._buf); }
      slice(start = 0, end = this.size, type = "") { const b = new Blob([], { type }); b._buf = this._buf.slice(start, end); b.size = b._buf.byteLength; return b; }
      stream() {
        if (typeof globalThis.ReadableStream === "undefined") {
          throw new Error("Blob.stream requires ReadableStream which is not available");
        }
        const buf = this._buf;
        return new globalThis.ReadableStream({
          start(controller) {
            if (buf.byteLength > 0) controller.enqueue(buf);
            controller.close();
          },
        });
      }
    }
    globalThis.Blob = Blob;
    if (typeof globalThis.File === "undefined") {
      class File extends Blob {
        constructor(parts, name, options = {}) { super(parts, options); this.name = String(name); this.lastModified = options.lastModified ?? Date.now(); }
      }
      globalThis.File = File;
    }
  }

  if (typeof globalThis.ReadableStream === "undefined") {
    class ReadableStream {
      constructor(underlying = {}, _strategy = {}) {
        this._underlying = underlying;
        this._chunks = [];
        this._closed = false;
        this._error = null;
        this._cancelled = false;
        this._locked = false;
        this._waiters = [];
        const wakeWaiters = () => {
          const waiters = this._waiters;
          this._waiters = [];
          for (const w of waiters) w();
        };
        const controller = {
          enqueue: (chunk) => {
            if (this._closed || this._cancelled) return;
            this._chunks.push(chunk);
            wakeWaiters();
          },
          close: () => { this._closed = true; wakeWaiters(); },
          error: (err) => { this._error = err; this._closed = true; wakeWaiters(); },
          get desiredSize() { return 1; },
        };
        try {
          if (typeof underlying.start === "function") {
            const r = underlying.start(controller);
            if (r && typeof r.then === "function") {
              r.then(
                () => {},
                (e) => { this._error = e; this._closed = true; wakeWaiters(); },
              );
            }
          }
        } catch (e) {
          this._error = e;
          this._closed = true;
        }
        this._controller = controller;
      }
      get locked() { return this._locked; }
      cancel(reason) {
        this._cancelled = true;
        this._closed = true;
        const waiters = this._waiters;
        this._waiters = [];
        for (const w of waiters) w();
        if (this._underlying && typeof this._underlying.cancel === "function") {
          try { return Promise.resolve(this._underlying.cancel(reason)); }
          catch (e) { return Promise.reject(e); }
        }
        return Promise.resolve();
      }
      getReader() {
        if (this._locked) throw new TypeError("ReadableStream is locked");
        this._locked = true;
        const stream = this;
        return {
          async read() {
            if (stream._error) throw stream._error;
            while (stream._chunks.length === 0 && !stream._closed) {
              if (typeof stream._underlying.pull === "function") {
                await stream._underlying.pull(stream._controller);
                if (stream._chunks.length > 0 || stream._closed) break;
              }
              await new Promise((resolve) => stream._waiters.push(resolve));
            }
            if (stream._error) throw stream._error;
            if (stream._chunks.length > 0) {
              return { value: stream._chunks.shift(), done: false };
            }
            return { value: undefined, done: true };
          },
          releaseLock() { stream._locked = false; },
          cancel(reason) { return stream.cancel(reason); },
          get closed() { return Promise.resolve(); },
        };
      }
      pipeTo(dest, options) {
        const opts = options || {};
        const preventClose = !!opts.preventClose;
        const preventAbort = !!opts.preventAbort;
        const signal = opts.signal;
        const reader = this.getReader();
        const writer = dest.getWriter ? dest.getWriter() : dest;
        return (async () => {
          try {
            while (true) {
              if (signal && signal.aborted) {
                throw signal.reason || new DOMException("aborted", "AbortError");
              }
              const { value, done } = await reader.read();
              if (done) break;
              if (writer.write) await writer.write(value);
            }
            if (!preventClose && writer.close) await writer.close();
          } catch (e) {
            if (!preventAbort && writer.abort) {
              try { await writer.abort(e); } catch (abortErr) { _trace("pipeTo writer.abort threw: " + abortErr); }
            }
            throw e;
          } finally {
            if (writer.releaseLock) {
              try { writer.releaseLock(); } catch { }
            }
          }
        })();
      }
      pipeThrough(transform) {
        this.pipeTo(transform.writable);
        return transform.readable;
      }
      tee() {
        if (this._locked) throw new TypeError("ReadableStream is locked");
        const reader = this.getReader();
        const branches = [];
        let pumping = false;
        let sourceDone = false;

        async function pump() {
          if (pumping || sourceDone) return;
          pumping = true;
          try {
            while (true) {
              if (branches.every((b) => b._closed || b._cancelled)) break;
              let r;
              try {
                r = await reader.read();
              } catch (err) {
                sourceDone = true;
                for (const b of branches) {
                  try { b._controller.error(err); } catch { }
                }
                return;
              }
              if (r.done) {
                sourceDone = true;
                for (const b of branches) {
                  try { b._controller.close(); } catch { }
                }
                return;
              }
              for (const b of branches) {
                if (!b._closed && !b._cancelled) {
                  try { b._controller.enqueue(r.value); } catch { }
                }
              }
              return;
            }
          } finally {
            pumping = false;
          }
        }

        const make = () => new ReadableStream({
          pull() { return pump(); },
          cancel() {
            if (branches.every((b) => b._closed || b._cancelled)) {
              try { reader.cancel(); } catch { }
            }
          },
        });
        const a = make();
        const b = make();
        branches.push(a, b);
        return [a, b];
      }
      [Symbol.asyncIterator]() {
        const reader = this.getReader();
        return {
          async next() { return reader.read(); },
          async return() { reader.releaseLock(); return { value: undefined, done: true }; },
        };
      }
    }
    globalThis.ReadableStream = ReadableStream;
  }

  if (typeof globalThis.WritableStream === "undefined") {
    class WritableStream {
      constructor(underlying = {}) {
        this._underlying = underlying;
        this._closed = false;
        this._locked = false;
        if (typeof underlying.start === "function") {
          try { underlying.start({ error: () => {} }); } catch (e) { _trace("WritableStream underlying.start threw: " + e); }
        }
      }
      get locked() { return this._locked; }
      getWriter() {
        if (this._locked) throw new TypeError("WritableStream is locked");
        this._locked = true;
        const stream = this;
        return {
          async write(chunk) {
            if (typeof stream._underlying.write === "function") {
              return stream._underlying.write(chunk);
            }
          },
          async close() {
            stream._closed = true;
            if (typeof stream._underlying.close === "function") {
              return stream._underlying.close();
            }
          },
          async abort(reason) {
            stream._closed = true;
            if (typeof stream._underlying.abort === "function") {
              return stream._underlying.abort(reason);
            }
          },
          releaseLock() { stream._locked = false; },
          get closed() { return Promise.resolve(); },
          get ready() { return Promise.resolve(); },
          get desiredSize() { return 1; },
        };
      }
      abort(reason) {
        this._closed = true;
        if (typeof this._underlying.abort === "function") {
          try { return Promise.resolve(this._underlying.abort(reason)); }
          catch (e) { return Promise.reject(e); }
        }
        return Promise.resolve();
      }
      close() { this._closed = true; return Promise.resolve(); }
    }
    globalThis.WritableStream = WritableStream;
  }

  if (typeof globalThis.TransformStream === "undefined") {
    class TransformStream {
      constructor(transformer = {}) {
        let readableController = null;
        const controller = {
          enqueue: (chunk) => { if (readableController) readableController.enqueue(chunk); },
          terminate: () => { if (readableController) readableController.close(); },
          error: (err) => { if (readableController) readableController.error(err); },
        };
        this.readable = new globalThis.ReadableStream({
          start(c) {
            readableController = c;
            if (typeof transformer.start === "function") {
              try { transformer.start(controller); } catch (e) { c.error(e); }
            }
          },
        });
        this.writable = new globalThis.WritableStream({
          async write(chunk) {
            if (typeof transformer.transform === "function") {
              await transformer.transform(chunk, controller);
            } else {
              controller.enqueue(chunk);
            }
          },
          async close() {
            if (typeof transformer.flush === "function") {
              await transformer.flush(controller);
            }
            if (readableController) readableController.close();
          },
          async abort(reason) {
            if (readableController) readableController.error(reason);
          },
        });
      }
    }
    globalThis.TransformStream = TransformStream;
  }

  if (typeof globalThis.ByteLengthQueuingStrategy === "undefined") {
    globalThis.ByteLengthQueuingStrategy = class ByteLengthQueuingStrategy {
      constructor({ highWaterMark = 1 } = {}) { this.highWaterMark = highWaterMark; }
      size(chunk) { return chunk?.byteLength ?? 0; }
    };
  }
  if (typeof globalThis.CountQueuingStrategy === "undefined") {
    globalThis.CountQueuingStrategy = class CountQueuingStrategy {
      constructor({ highWaterMark = 1 } = {}) { this.highWaterMark = highWaterMark; }
      size() { return 1; }
    };
  }
})();
