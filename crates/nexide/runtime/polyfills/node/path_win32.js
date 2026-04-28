"use strict";

const { win32 } = require("node:path");
if (!win32) {
  throw new Error("nexide: node:path/win32 unavailable - path polyfill missing win32 export");
}
module.exports = win32;
