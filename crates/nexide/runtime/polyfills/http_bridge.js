// Handler stack + synthetic request/response builders driving
// `globalThis.__nexide.__dispatch`.
//
// Splits two responsibilities:
//
//   - The handler **stack** (`pushHandler`/`popHandler`/`setHandler`)
//     decides which JS function should service the next request slot.
//     Stacks honour LIFO so a freshly-listened server preempts older
//     ones; closing the top server hands traffic back to the previous
//     entry. This is intentionally tiny and Rust-agnostic — Rust just
//     calls `__dispatch()` once per planted slot.
//
//   - The synthetic `req`/`res` objects exposed to the registered
//     handler are infrastructure only. They wrap raw nexide ops with
//     a Node-shaped event surface (`req.on('data'/'end')` +
//     `res.writeHead/write/end`) that higher-level modules
//     (`node:http`) adapt to full-fidelity `IncomingMessage` /
//     `ServerResponse` instances.
//
// Compatibility shim: an entrypoint that predates `node:http`
// (`example/nexide_app.mjs`) accesses `globalThis.http.createServer`.
// At polyfill install time we lazily wire that to
// `require('node:http')` once the CJS loader is present, preserving
// drop-in behaviour without keeping a duplicate synthetic
// implementation.

((globalThis) => {
  "use strict";

  const ops = Nexide.core.ops;
  const nexide = globalThis.__nexide;
  if (!nexide || nexide.__httpBridge) {
    return;
  }

  const stack = [];
  let nextToken = 1;

  function buildIncoming(idx, gen) {
    const meta = ops.op_nexide_get_meta(idx, gen);

    let cachedRawHeaders = null;
    let cachedHeaders = null;
    let cachedRawHeadersFlat = null;

    function rawHeaders() {
      if (cachedRawHeaders === null) {
        cachedRawHeaders = ops.op_nexide_get_headers(idx, gen);
      }
      return cachedRawHeaders;
    }

    const dataListeners = [];
    const endListeners = [];
    const errorListeners = [];
    const bufferedChunks = [];
    let pumped = false;
    let ended = false;

    const incoming = {
      method: meta.method,
      url: meta.uri,
      httpVersion: "1.1",

      get headers() {
        if (cachedHeaders === null) {
          cachedHeaders = Object.create(null);
          const raw = rawHeaders();
          for (let i = 0; i < raw.length; i++) {
            cachedHeaders[raw[i].name] = raw[i].value;
          }
        }
        return cachedHeaders;
      },

      get rawHeaders() {
        if (cachedRawHeadersFlat === null) {
          const raw = rawHeaders();
          cachedRawHeadersFlat = new Array(raw.length * 2);
          for (let i = 0; i < raw.length; i++) {
            cachedRawHeadersFlat[i * 2] = raw[i].name;
            cachedRawHeadersFlat[i * 2 + 1] = raw[i].value;
          }
        }
        return cachedRawHeadersFlat;
      },

      on(event, cb) {
        if (event === "data") {
          dataListeners.push(cb);
          if (bufferedChunks.length) {
            const replay = bufferedChunks.slice();
            queueMicrotask(() => {
              for (const chunk of replay) cb(chunk);
            });
          }
          if (!pumped) queueMicrotask(() => incoming.__pump());
        } else if (event === "end") {
          endListeners.push(cb);
          if (ended) {
            queueMicrotask(() => cb());
          } else if (!pumped) {
            queueMicrotask(() => incoming.__pump());
          }
        } else if (event === "error") {
          errorListeners.push(cb);
        }
        return incoming;
      },

      once(event, cb) {
        const wrap = (...args) => {
          incoming.off(event, wrap);
          cb(...args);
        };
        return incoming.on(event, wrap);
      },

      off(event, cb) {
        const arr =
          event === "data"
            ? dataListeners
            : event === "end"
              ? endListeners
              : event === "error"
                ? errorListeners
                : null;
        if (!arr) return incoming;
        const i = arr.indexOf(cb);
        if (i >= 0) arr.splice(i, 1);
        return incoming;
      },

      __pump() {
        if (pumped) return;
        pumped = true;
        const chunkSize = 8192;
        const buf = new Uint8Array(chunkSize);
        for (;;) {
          const n = ops.op_nexide_read_body(idx, gen, buf);
          if (n === 0) break;
          const slice = buf.slice(0, n);
          bufferedChunks.push(slice);
          for (const cb of dataListeners.slice()) cb(slice);
        }
        ended = true;
        for (const cb of endListeners.slice()) cb();
      },
    };

    return incoming;
  }

  function asArray(headers) {
    if (!headers) return [];
    if (Array.isArray(headers)) {
      if (headers.length && Array.isArray(headers[0])) return headers.slice();
      const out = [];
      for (let i = 0; i + 1 < headers.length; i += 2) {
        out.push([headers[i], headers[i + 1]]);
      }
      return out;
    }
    return Object.entries(headers);
  }

  function asciiBytes(str) {
    const s = String(str);
    const out = new Uint8Array(s.length);
    for (let i = 0; i < s.length; i++) {
      out[i] = s.charCodeAt(i) & 0xff;
    }
    return out;
  }

  const EMPTY_BODY = new Uint8Array(0);

  function buildResponse(idx, gen) {
    let headSent = false;
    let ended = false;
    const queuedChunks = [];
    let pendingHead = null;

    function flushHead() {
      if (headSent || pendingHead === null) return;
      ops.op_nexide_send_head(idx, gen, {
        status: pendingHead.status,
        headers: pendingHead.headers,
      });
      headSent = true;
      for (const chunk of queuedChunks) {
        ops.op_nexide_send_chunk(idx, gen, chunk);
      }
      queuedChunks.length = 0;
    }

    const res = {
      statusCode: 200,
      statusMessage: "OK",

      writeHead(status, statusMessageOrHeaders, maybeHeaders) {
        if (ended) throw new Error("writeHead after end()");
        if (headSent) throw new Error("writeHead called twice");
        let headers;
        if (
          typeof statusMessageOrHeaders === "string" ||
          statusMessageOrHeaders === undefined
        ) {
          headers = asArray(maybeHeaders);
        } else {
          headers = asArray(statusMessageOrHeaders);
        }
        pendingHead = { status, headers };
        res.statusCode = status;
        return res;
      },

      setHeader(name, value) {
        if (headSent) throw new Error("setHeader after head sent");
        if (pendingHead === null) {
          pendingHead = { status: res.statusCode, headers: [] };
        }
        pendingHead.headers.push([String(name).toLowerCase(), String(value)]);
        return res;
      },

      write(chunk) {
        if (ended) throw new Error("write after end()");
        if (pendingHead === null) {
          pendingHead = { status: res.statusCode, headers: [] };
        }
        const buf = chunk instanceof Uint8Array ? chunk : asciiBytes(chunk);
        if (!headSent) {
          flushHead();
        }
        ops.op_nexide_send_chunk(idx, gen, buf);
        return true;
      },

      end(chunk) {
        if (ended) return;
        if (pendingHead === null) {
          pendingHead = { status: res.statusCode, headers: [] };
        }

        if (!headSent) {
          let body;
          if (chunk === undefined || chunk === null) {
            body = EMPTY_BODY;
          } else if (chunk instanceof Uint8Array) {
            body = chunk;
          } else {
            body = asciiBytes(chunk);
          }
          if (queuedChunks.length === 0) {
            ops.op_nexide_send_response(
              idx,
              gen,
              pendingHead.status,
              pendingHead.headers,
              body,
            );
            headSent = true;
            ended = true;
            return;
          }
        }

        flushHead();
        if (chunk !== undefined && chunk !== null) {
          const buf = chunk instanceof Uint8Array ? chunk : asciiBytes(chunk);
          ops.op_nexide_send_chunk(idx, gen, buf);
        }
        ops.op_nexide_send_end(idx, gen);
        ended = true;
      },

      __isEnded() {
        return ended;
      },
    };

    return res;
  }

  nexide.pushHandler = function (fn) {
    if (typeof fn !== "function") {
      throw new TypeError("pushHandler expects a function");
    }
    const token = nextToken++;
    stack.push({ token, fn });
    return token;
  };

  nexide.popHandler = function (token) {
    for (let i = stack.length - 1; i >= 0; i--) {
      if (stack[i].token === token) {
        stack.splice(i, 1);
        return true;
      }
    }
    return false;
  };

  nexide.activeHandlerToken = function () {
    return stack.length ? stack[stack.length - 1].token : null;
  };

  nexide.setHandler = function (fn) {
    stack.length = 0;
    if (typeof fn === "function") {
      stack.push({ token: nextToken++, fn });
    }
  };

  nexide.__dispatch = function (idx, gen) {
    const top = stack[stack.length - 1];
    if (!top) {
      throw new Error("nexide: no handler registered");
    }
    const req = buildIncoming(idx, gen);
    const res = buildResponse(idx, gen);

    let ret;
    let threw = false;
    let thrown;
    try {
      ret = top.fn(req, res);
    } catch (err) {
      threw = true;
      thrown = err;
    }

    if (
      !threw &&
      res.__isEnded() &&
      (ret === undefined || ret === null || typeof ret.then !== "function")
    ) {
      return;
    }

    let handlerPromise;
    if (threw) {
      handlerPromise = Promise.reject(thrown);
    } else if (ret && typeof ret.then === "function") {
      handlerPromise = ret;
    } else {
      handlerPromise = Promise.resolve(ret);
    }

    return handlerPromise.then(
      () => {
        if (!res.__isEnded()) {
          try { res.end(); } catch { }
        }
      },
      (err) => {
        if (!res.__isEnded()) {
          try { res.end(); } catch { }
        }
        throw err;
      },
    );
  };

  nexide.__httpBridge = true;

  let httpModuleCache = null;
  function loadHttp() {
    if (httpModuleCache) return httpModuleCache;
    if (typeof globalThis.require !== "function") return null;
    httpModuleCache = globalThis.require("node:http");
    return httpModuleCache;
  }
  Object.defineProperty(globalThis, "http", {
    configurable: true,
    enumerable: false,
    get() {
      const mod = loadHttp();
      if (!mod) throw new Error("nexide: globalThis.http requires the CJS loader");
      return mod;
    },
  });
  Object.defineProperty(globalThis, "node_http", {
    configurable: true,
    enumerable: false,
    get() {
      return globalThis.http;
    },
  });
})(globalThis);
