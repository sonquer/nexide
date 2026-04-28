"use strict";

// node:fs/promises — re-export of the `promises` namespace from
// `node:fs`. Many callers `require('node:fs/promises')` directly.

const fs = require("node:fs");
module.exports = fs.promises;
module.exports.constants = fs.constants;
