"use strict";

// node:stream/consumers - drain a readable (Node Readable, AsyncIterable
// or WHATWG ReadableStream) into a Buffer / string / JSON / ArrayBuffer
// / Blob. Heavily used by undici, node-fetch@3 and formidable.

const { Buffer } = require("node:buffer");

async function collect(input) {
  if (input == null) return [];

  if (typeof globalThis.ReadableStream !== "undefined"
      && input instanceof globalThis.ReadableStream) {
    const reader = input.getReader();
    const chunks = [];
    try {
      // eslint-disable-next-line no-constant-condition
      while (true) {
        const { value, done } = await reader.read();
        if (done) break;
        if (value !== undefined) chunks.push(value);
      }
    } finally {
      try { reader.releaseLock(); } catch { /* noop */ }
    }
    return chunks;
  }

  if (typeof input[Symbol.asyncIterator] === "function") {
    const chunks = [];
    for await (const chunk of input) chunks.push(chunk);
    return chunks;
  }

  if (typeof input.on === "function") {
    return new Promise((resolve, reject) => {
      const chunks = [];
      input.on("data", (c) => chunks.push(c));
      input.on("end", () => resolve(chunks));
      input.on("error", reject);
    });
  }

  throw new TypeError("stream/consumers: unsupported input");
}

function toBuffer(chunks) {
  const bufs = chunks.map((c) =>
    Buffer.isBuffer(c)
      ? c
      : c instanceof Uint8Array
        ? Buffer.from(c.buffer, c.byteOffset, c.byteLength)
        : typeof c === "string"
          ? Buffer.from(c, "utf8")
          : Buffer.from(c),
  );
  return Buffer.concat(bufs);
}

async function buffer(input) {
  return toBuffer(await collect(input));
}

async function text(input) {
  return (await buffer(input)).toString("utf8");
}

async function json(input) {
  return JSON.parse(await text(input));
}

async function arrayBuffer(input) {
  const b = await buffer(input);
  return b.buffer.slice(b.byteOffset, b.byteOffset + b.byteLength);
}

async function blob(input) {
  if (typeof globalThis.Blob === "undefined") {
    throw new Error("stream/consumers blob: globalThis.Blob is unavailable");
  }
  const b = await buffer(input);
  return new globalThis.Blob([b]);
}

module.exports = { buffer, text, json, arrayBuffer, blob };
