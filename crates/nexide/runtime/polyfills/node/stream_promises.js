"use strict";

// node:stream/promises - promise-returning wrappers over the
// callback-style APIs in node:stream. Mirrors the Node 16+ surface used
// by libraries that prefer `await pipeline(...)` over the legacy form.

const stream = require("node:stream");

function pipeline(...args) {
  if (typeof args[args.length - 1] === "function") {
    throw new TypeError(
      "stream/promises pipeline does not accept a callback; use node:stream",
    );
  }
  return stream.pipeline(...args);
}

function finished(s, opts) {
  return new Promise((resolve, reject) => {
    stream.finished(s, (err) => (err ? reject(err) : resolve()));
  });
}

module.exports = { pipeline, finished };
