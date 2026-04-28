"use strict";

// node:assert/strict - mirrors Node's behaviour where the strict export
// is identical to the regular `assert` module (nexide's assert polyfill
// already uses strict-equality semantics).

module.exports = require("node:assert");
