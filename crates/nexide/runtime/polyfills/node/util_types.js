"use strict";

// node:util/types - subset re-export of util.types. Some libraries
// (graceful-fs, ajv, fastify schema compilers) `require
// ("node:util/types")` directly to avoid pulling the full util module.

const util = require("node:util");
module.exports = util.types;
