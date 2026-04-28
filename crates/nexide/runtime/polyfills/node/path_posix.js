"use strict";

const { posix } = require("node:path");
if (!posix) {
  throw new Error("nexide: node:path/posix unavailable - path polyfill missing posix export");
}
module.exports = posix;
