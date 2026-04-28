"use strict";

/**
 * JavaScript façade over the host-side request bridge.
 *
 * Exposes `globalThis.__nexide`, a small object that wraps the raw
 * `Nexide.core.ops.op_nexide_*` callbacks installed by Rust. Every
 * façade method takes the opaque `(idx, gen)` request id pair as its
 * leading arguments so concurrent in-flight requests can share the
 * same V8 isolate without stomping on one another's slots.
 *
 * The script is loaded once per isolate and is idempotent - re-running
 * it (for example from tests that boot multiple isolates from the
 * same process) leaves the existing bridge installed untouched.
 */

((globalThis) => {
  if (globalThis.__nexide && globalThis.__nexide.__nexideBridge) {
    return;
  }

  const ops = Nexide.core.ops;

  const bridge = {
    __nexideBridge: true,

    /** Returns the request method, URL and remote address. */
    getMeta(idx, gen) {
      return ops.op_nexide_get_meta(idx, gen);
    },

    /** Returns the request headers as an array of `[name, value]` pairs. */
    getHeaders(idx, gen) {
      return ops.op_nexide_get_headers(idx, gen);
    },

    /**
     * Fills `buffer` with the next chunk of the request body and
     * returns the number of bytes written, or `0` on EOF.
     */
    readBody(idx, gen, buffer) {
      return ops.op_nexide_read_body(idx, gen, buffer);
    },

    /** Sends the response status line and headers. */
    sendHead(idx, gen, status, headers) {
      ops.op_nexide_send_head(idx, gen, { status, headers: headers || [] });
    },

    /** Streams a single response body chunk. */
    sendChunk(idx, gen, chunk) {
      ops.op_nexide_send_chunk(idx, gen, chunk);
    },

    /** Marks the response stream as finished. */
    sendEnd(idx, gen) {
      ops.op_nexide_send_end(idx, gen);
    },

    /**
     * Aborts the request with `message` so the host shield can map
     * the failure to a `502` and free the dispatch slot.
     */
    finishError(idx, gen, message) {
      ops.op_nexide_finish_error(idx, gen, String(message));
    },

    /**
     * Long-lived async loop installed by Rust at worker boot.
     *
     * Awaits the next request id pair (or batch) from the per-isolate
     * `RequestQueue`, then fires `__dispatch(idx, gen)` *without*
     * awaiting it - concurrent in-flight requests share one isolate
     * because the V8 microtask queue interleaves their async
     * continuations naturally.
     *
     * `batchCap` selects the pump strategy:
     *   * `undefined`, `null`, `0`, `1` - serial pump (one
     *     `op_nexide_pop_request` per request).
     *   * `>= 2` - batched pump (`op_nexide_pop_request_batch(cap)`)
     *     dispatches every id in the returned slice within the same
     *     microtask cycle, amortising per-request op crossing under
     *     sustained load.
     *
     * Both synchronous throws and rejected handler promises are
     * funnelled into `op_nexide_finish_error`, which settles the
     * pending dispatcher oneshot with the handler-failure variant so
     * the HTTP shield can map the failure to a `502` and free the
     * slot. Without this path the slot would leak until the worker
     * was recycled.
     */
    __startPump(batchCap) {
      if (bridge.__pumpStarted) return;
      bridge.__pumpStarted = true;

      const cap = (batchCap | 0) > 1 ? (batchCap | 0) : 0;

      function dispatchOne(idx, gen) {
        let ret;
        try {
          ret = bridge.__dispatch ? bridge.__dispatch(idx, gen) : undefined;
        } catch (err) {
          bridge.__finalizeError(idx, gen, err);
          return;
        }
        if (ret && typeof ret.then === "function") {
          ret.then(undefined, (err) => {
            bridge.__finalizeError(idx, gen, err);
          });
        }
      }

      if (cap === 0) {
        (async function pump() {
          for (;;) {
            let pair;
            try {
              pair = await ops.op_nexide_pop_request();
            } catch (_err) {
              return;
            }
            dispatchOne(pair[0], pair[1]);
          }
        })();
      } else {
        (async function pumpBatch() {
          for (;;) {
            let batch;
            try {
              batch = await ops.op_nexide_pop_request_batch(cap);
            } catch (_err) {
              return;
            }
            for (let i = 0; i < batch.length; i++) {
              const pair = batch[i];
              dispatchOne(pair[0], pair[1]);
            }
            for (;;) {
              let drained;
              try {
                drained = ops.op_nexide_try_pop_request_batch(cap);
              } catch (_err) {
                drained = null;
              }
              if (!drained || drained.length === 0) break;
              for (let j = 0; j < drained.length; j++) {
                const pair = drained[j];
                dispatchOne(pair[0], pair[1]);
              }
            }
          }
        })();
      }
    },

    /**
     * Translates a thrown / rejected value into the string payload
     * accepted by `op_nexide_finish_error` and forwards it.
     *
     * Captures `name`, `message` and the first eight stack frames -
     * enough for diagnostics yet bounded so a pathological error
     * cannot inflate isolate memory by reporting a multi-megabyte
     * stack. If `op_nexide_finish_error` itself throws (the
     * dispatcher slot was already settled, for example) the inner
     * failure is logged through `op_nexide_log` at error level so
     * the host always observes it - `console` is not consulted
     * because user code may have replaced or removed it.
     *
     * If even the logging op refuses to fire, the host bridge is
     * fundamentally broken: the slot will leak from the caller's
     * point of view and we have no way to surface diagnostics. In
     * that case we request a non-zero process exit so the operator
     * (and the supervisor) sees the failure rather than continuing
     * to silently corrupt request state.
     */
    __finalizeError(idx, gen, err) {
      const formatted = bridge.__formatError(err);
      try {
        ops.op_nexide_finish_error(idx, gen, formatted);
      } catch (finalizeErr) {
        try {
          ops.op_nexide_log(
            4,
            "nexide pump: finish_error failed: " +
              bridge.__formatError(finalizeErr),
          );
        } catch (logErr) {
          try {
            ops.op_process_exit(70);
          } catch (_exitErr) {
            throw logErr;
          }
        }
      }
    },

    /**
     * Renders an arbitrary error value as the diagnostic string sent
     * back to the host. Non-object values are coerced with `String()`,
     * `null`/`undefined` become a sentinel literal, and Error-shaped
     * objects emit `name: message\n<top 8 stack frames>`.
     */
    __formatError(err) {
      if (err == null) return "handler error: <null>";
      if (typeof err !== "object") return String(err);
      const name = typeof err.name === "string" ? err.name : "Error";
      const message = typeof err.message === "string" ? err.message : "";
      const stack = typeof err.stack === "string"
        ? err.stack.split("\n").slice(0, 8).join("\n")
        : "";
      return stack
        ? `${name}: ${message}\n${stack}`
        : `${name}: ${message}`;
    },
  };

  Object.defineProperty(bridge, "__nexideBridge", {
    value: true,
    enumerable: false,
    configurable: false,
    writable: false,
  });
  globalThis.__nexide = bridge;
})(globalThis);
