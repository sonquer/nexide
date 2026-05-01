"use strict";
// CommonJS loader installed on `globalThis.require`.
//
// Resolution and source reads are delegated to Rust ops so that the
// sandboxed `FsResolver` remains the single source of truth for file
// access. Module instances are cached per absolute specifier; cyclic
// requires obey Node.js semantics (partial `module.exports` is
// returned to the cycle's first observer).

(() => {
  if (globalThis.__nexideCjs) {
    return;
  }

  const ops = Nexide.core.ops;
  const cache = new Map();
  const moduleStack = [];

  function dirnameOf(spec) {
    if (typeof spec !== "string" || spec.length === 0) return "";
    if (spec.startsWith("node:")) return spec;
    let i = spec.length - 1;
    while (i > 0 && spec[i] !== "/" && spec[i] !== "\\") i--;
    if (i <= 0) return spec[0] === "/" || spec[0] === "\\" ? spec[0] : "";
    return spec.slice(0, i);
  }

  function basenameOf(spec) {
    if (typeof spec !== "string" || spec.length === 0) return "";
    let i = spec.length - 1;
    while (i >= 0 && spec[i] !== "/" && spec[i] !== "\\") i--;
    return spec.slice(i + 1);
  }

  function compileWrapper(source, specifier) {
    const fn = ops.op_cjs_compile_function(source, specifier);
    return function (exports, require, module, __filename, __dirname) {
      moduleStack.push(specifier);
      try {
        return fn(exports, require, module, __filename, __dirname);
      } finally {
        moduleStack.pop();
      }
    };
  }

  function makeRequire(parent) {
    const fn = (request) => loadModule(parent, request);
    fn.cache = cache;
    fn.resolve = (request) => ops.op_cjs_resolve(parent, request);
    fn.resolve.paths = () => null;
    fn.extensions = Object.create(null);
    fn.main = undefined;
    return fn;
  }

  function tagError(err) {
    if (err && typeof err.message === "string" && !err.code) {
      const m = err.message.match(/^([A-Z][A-Z0-9_]+):\s/);
      if (m) err.code = m[1];
    }
    return err;
  }

  function loadModule(parent, request) {
    let specifier;
    try { specifier = ops.op_cjs_resolve(parent, request); }
    catch (e) { throw tagError(e); }
    const cached = cache.get(specifier);
    if (cached) return cached.exports;

    let result;
    try { result = ops.op_cjs_read_source(specifier); }
    catch (e) { throw tagError(e); }
    const source = result[0];
    const kind = result[1];

    const module = {
      exports: {},
      id: specifier,
      filename: specifier.startsWith("node:") ? specifier : specifier,
      loaded: false,
      children: [],
      parent: parent === "<root>" ? null : parent,
    };
    cache.set(specifier, module);

    try {
      if (kind === 1) {
        module.exports = JSON.parse(source);
      } else if (kind === 3) {
        module.exports = ops.op_napi_load(source);
      } else {
        const fn = compileWrapper(source, specifier);
        const __filename = specifier;
        const __dirname = dirnameOf(specifier);
        fn(module.exports, makeRequire(specifier), module, __filename, __dirname);
      }
      module.loaded = true;
      return module.exports;
    } catch (err) {
      cache.delete(specifier);
      throw err;
    }
  }

  function buildNamespace(exports) {
    if (exports && typeof exports === "object" && exports.__esModule) {
      return exports;
    }
    const ns = Object.create(null);
    if (exports !== null && exports !== undefined) {
      if (typeof exports === "object" || typeof exports === "function") {
        for (const k of Object.keys(exports)) {
          try { ns[k] = exports[k]; } catch (_) {}
        }
      }
    }
    ns.default = exports;
    return ns;
  }

  function dynamicImport(specifier, referrer) {
    let parent;
    if (typeof referrer === "string" && referrer.length > 0) {
      parent = referrer;
    } else if (moduleStack.length > 0) {
      parent = moduleStack[moduleStack.length - 1];
    } else {
      parent = ops.op_cjs_root_parent();
    }
    const exports = loadModule(parent, specifier);
    return buildNamespace(exports);
  }

  Object.defineProperty(globalThis, "__nexideCjs", {
    value: {
      load: loadModule,
      cache,
      makeRequire,
      dirnameOf,
      basenameOf,
      dynamicImport,
    },
    enumerable: false,
    writable: false,
    configurable: false,
  });

  const rootParent = ops.op_cjs_root_parent();
  globalThis.require = makeRequire(rootParent);
})();
