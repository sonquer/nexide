// node:module - minimal compat surface to let Next.js' require-hook
// machinery load. The hook itself monkey-patches `Module.prototype.require`
// and `Module._resolveFilename`; nexide does not honour the patches but
// exposing the shape lets the hook installer execute without throwing.

(function () {
  function Module(id, parent) {
    this.id = id || "";
    this.exports = {};
    this.parent = parent || null;
    this.filename = null;
    this.loaded = false;
    this.children = [];
    this.paths = [];
  }

  Module.prototype.require = function (specifier) {
    return globalThis.require(specifier);
  };

  Module._cache = Object.create(null);
  Module._extensions = Object.create(null);
  Module._resolveFilename = function (request) {
    return request;
  };

  function createRequire(filename) {
    if (typeof filename !== "string") {
      throw new TypeError("createRequire requires a string filename");
    }
    return globalThis.require;
  }

  const builtinModules = [
    "assert", "async_hooks", "buffer", "crypto", "events",
    "fs", "fs/promises", "http", "module", "net", "os",
    "path", "perf_hooks", "process", "querystring", "stream",
    "tty", "url", "util", "zlib",
  ];

  module.exports = Module;
  module.exports.Module = Module;
  module.exports.createRequire = createRequire;
  module.exports.builtinModules = builtinModules;
  module.exports.isBuiltin = function (name) {
    if (typeof name !== "string") return false;
    const bare = name.startsWith("node:") ? name.slice(5) : name;
    return builtinModules.includes(bare);
  };
  module.exports.syncBuiltinESMExports = function () {};
})();
