"use strict";

// node:fs - sync + promise APIs backed by `nexide_fs_ops`.
// Streams are simple wrappers over readFileSync/writeFileSync - adequate
// for the request shapes Next.js standalone exercises (assets,
// `.next/server/...` static reads).

const ops = Nexide.core.ops;
const Buffer = globalThis.Buffer;
const { Readable, Writable } = require("node:stream");
const path = require("node:path");

function toBuf(input, encoding) {
  if (input instanceof Uint8Array) return input;
  if (typeof input === "string") return Buffer.from(input, encoding || "utf8");
  throw new TypeError("fs expects Buffer/Uint8Array/string");
}

function decodeMaybe(buf, encoding) {
  if (!encoding || encoding === "buffer") return Buffer.from(buf);
  return Buffer.from(buf).toString(encoding);
}

function pathStr(p) {
  if (typeof p === "string") return p;
  if (p instanceof URL) return require("node:url").fileURLToPath(p);
  if (p instanceof Uint8Array) return Buffer.from(p).toString();
  throw new TypeError("path must be string|URL|Buffer");
}

function makeStats(raw) {
  const mtimeMs = raw.mtime_ms ?? 0;
  const atimeMs = raw.atime_ms ?? mtimeMs;
  const ctimeMs = raw.ctime_ms ?? mtimeMs;
  const birthtimeMs = raw.birthtime_ms ?? ctimeMs;
  return {
    size: raw.size,
    mtimeMs,
    atimeMs,
    ctimeMs,
    birthtimeMs,
    mtime: new Date(mtimeMs),
    atime: new Date(atimeMs),
    ctime: new Date(ctimeMs),
    birthtime: new Date(birthtimeMs),
    mode: raw.mode,
    uid: raw.uid ?? 0,
    gid: raw.gid ?? 0,
    ino: raw.ino ?? 0,
    dev: raw.dev ?? 0,
    nlink: raw.nlink ?? 1,
    rdev: raw.rdev ?? 0,
    blksize: raw.blksize ?? 4096,
    blocks: raw.blocks ?? 0,
    isFile: () => raw.is_file,
    isDirectory: () => raw.is_dir,
    isSymbolicLink: () => raw.is_symlink,
    isBlockDevice: () => false,
    isCharacterDevice: () => false,
    isFIFO: () => false,
    isSocket: () => false,
  };
}

const FS_CODES = [
  "EACCES", "ENOENT", "EEXIST", "ENOTDIR", "EISDIR",
  "EINVAL", "EPERM", "ENOSYS", "EIO", "ENOTEMPTY",
];
function fsCall(fn, ...args) {
  try { return fn(...args); } catch (raw) {
    let err = raw;
    if (err === undefined || err === null) {
      err = new Error("fs op failed");
    } else if (typeof err !== "object") {
      err = new Error(String(err));
    }
    if (!err.code) {
      const msg = ((err.message || "") + "");
      for (const c of FS_CODES) {
        if (msg.startsWith(c) || err.name === c) { err.code = c; break; }
      }
      if (!err.code && raw && typeof raw === "object" && raw.name) {
        for (const c of FS_CODES) if (raw.name === c) { err.code = c; break; }
      }
    }
    throw err;
  }
}

function readFileSync(p, opts) {
  const encoding = typeof opts === "string" ? opts : opts && opts.encoding;
  const buf = fsCall(ops.op_fs_read, pathStr(p));
  return decodeMaybe(buf, encoding);
}
function writeFileSync(p, data, opts) {
  const encoding = typeof opts === "string" ? opts : opts && opts.encoding;
  fsCall(ops.op_fs_write, pathStr(p), toBuf(data, encoding));
}
function existsSync(p) {
  try { return ops.op_fs_exists(pathStr(p)); } catch { return false; }
}
function statSync(p) { return makeStats(fsCall(ops.op_fs_stat, pathStr(p), true)); }
function lstatSync(p) { return makeStats(fsCall(ops.op_fs_stat, pathStr(p), false)); }
function realpathSync(p) { return fsCall(ops.op_fs_realpath, pathStr(p)); }
function readdirSync(p, opts) {
  const entries = fsCall(ops.op_fs_readdir, pathStr(p));
  if (opts && opts.withFileTypes) {
    return entries.map((e) => ({
      name: e.name,
      isFile: () => !e.is_dir && !e.is_symlink,
      isDirectory: () => e.is_dir,
      isSymbolicLink: () => e.is_symlink,
    }));
  }
  return entries.map((e) => e.name);
}
function mkdirSync(p, opts) {
  const recursive = typeof opts === "object" && opts ? Boolean(opts.recursive) : false;
  fsCall(ops.op_fs_mkdir, pathStr(p), recursive);
}
function rmSync(p, opts) {
  const recursive = typeof opts === "object" && opts ? Boolean(opts.recursive) : false;
  fsCall(ops.op_fs_rm, pathStr(p), recursive);
}
function unlinkSync(p) { fsCall(ops.op_fs_rm, pathStr(p), false); }
function copyFileSync(src, dst) { fsCall(ops.op_fs_copy, pathStr(src), pathStr(dst)); }
function readlinkSync(p) { return fsCall(ops.op_fs_readlink, pathStr(p)); }
function renameSync(src, dst) { fsCall(ops.op_fs_rename, pathStr(src), pathStr(dst)); }
function chmodSync(p, mode) { fsCall(ops.op_fs_chmod, pathStr(p), Number(mode) >>> 0); }
function symlinkSync(target, linkPath) { fsCall(ops.op_fs_symlink, String(target), pathStr(linkPath)); }
function linkSync(existingPath, newPath) { fsCall(ops.op_fs_link, pathStr(existingPath), pathStr(newPath)); }
function truncateSync(p, len) { fsCall(ops.op_fs_truncate, pathStr(p), Number(len) || 0); }
function utimesSync(p, atime, mtime) {
  const toMs = (t) => t instanceof Date ? t.getTime() : Number(t) * 1000;
  fsCall(ops.op_fs_utimes, pathStr(p), toMs(atime), toMs(mtime));
}

function appendFileSync(p, data, opts) {
  const encoding = typeof opts === "string" ? opts : opts && opts.encoding;
  fsCall(ops.op_fs_append, pathStr(p), toBuf(data, encoding));
}

// Async ops use tokio::fs and don't block the JS pump thread - the
// promise APIs prefer them when available, falling back to the sync
// op via queueMicrotask if the async op isn't installed (older
// embedder builds).
function asyncOr(asyncFnName, syncFn) {
  const asyncFn = ops[asyncFnName];
  if (typeof asyncFn !== "function") {
    return (...args) => new Promise((resolve, reject) => {
      queueMicrotask(() => {
        try { resolve(syncFn(...args)); } catch (err) { reject(err); }
      });
    });
  }
  return (...args) => Promise.resolve(asyncFn(...args)).catch((err) => {
    if (err && !err.code) {
      const msg = String(err.message || "");
      for (const c of FS_CODES) {
        if (msg.startsWith(c)) { err.code = c; break; }
      }
    }
    throw err;
  });
}

const readFileAsync = asyncOr("op_fs_read_async", (p, opts) => {
  const encoding = typeof opts === "string" ? opts : opts && opts.encoding;
  return decodeMaybe(ops.op_fs_read(pathStr(p)), encoding);
});
const writeFileAsync = asyncOr("op_fs_write_async", (p, data, opts) => {
  const encoding = typeof opts === "string" ? opts : opts && opts.encoding;
  ops.op_fs_write(pathStr(p), toBuf(data, encoding));
});
const appendFileAsync = asyncOr("op_fs_append_async", (p, data, opts) => {
  const encoding = typeof opts === "string" ? opts : opts && opts.encoding;
  ops.op_fs_append(pathStr(p), toBuf(data, encoding));
});
const statAsyncRaw = asyncOr("op_fs_stat_async", (p, follow) =>
  ops.op_fs_stat(pathStr(p), follow));
const readdirAsyncRaw = asyncOr("op_fs_readdir_async", (p) =>
  ops.op_fs_readdir(pathStr(p)));
const mkdirAsync = asyncOr("op_fs_mkdir_async", (p, recursive) =>
  ops.op_fs_mkdir(pathStr(p), recursive));
const rmAsync = asyncOr("op_fs_rm_async", (p, recursive) =>
  ops.op_fs_rm(pathStr(p), recursive));
const copyFileAsync = asyncOr("op_fs_copy_async", (src, dst) =>
  ops.op_fs_copy(pathStr(src), pathStr(dst)));
const renameAsync = asyncOr("op_fs_rename_async", (src, dst) =>
  ops.op_fs_rename(pathStr(src), pathStr(dst)));
const realpathAsync = asyncOr("op_fs_realpath_async", (p) =>
  ops.op_fs_realpath(pathStr(p)));

function readFileWithEncoding(p, opts) {
  const encoding = typeof opts === "string" ? opts : opts && opts.encoding;
  return readFileAsync(pathStr(p)).then((bytes) => decodeMaybe(bytes, encoding));
}
function writeFileWithEncoding(p, data, opts) {
  const encoding = typeof opts === "string" ? opts : opts && opts.encoding;
  return writeFileAsync(pathStr(p), toBuf(data, encoding));
}
function appendFileWithEncoding(p, data, opts) {
  const encoding = typeof opts === "string" ? opts : opts && opts.encoding;
  return appendFileAsync(pathStr(p), toBuf(data, encoding));
}

function notAvailable(name) {
  const err = new Error(
    `${name} is not available in nexide; filesystem change notifications are not supported in this runtime`,
  );
  err.code = "ERR_NOT_AVAILABLE"; throw err;
}

function createReadStream(p, opts) {
  const encoding = opts && opts.encoding;
  const buf = ops.op_fs_read(pathStr(p));
  const stream = new Readable();
  queueMicrotask(() => {
    stream.push(encoding ? Buffer.from(buf).toString(encoding) : Buffer.from(buf));
    stream.push(null);
  });
  return stream;
}
function createWriteStream(p) {
  const target = pathStr(p);
  const chunks = [];
  return new Writable({
    write(chunk, _enc, cb) { chunks.push(toBuf(chunk)); cb(); },
    final(cb) {
      let total = 0; for (const c of chunks) total += c.byteLength;
      const out = new Uint8Array(total);
      let off = 0; for (const c of chunks) { out.set(c, off); off += c.byteLength; }
      try { ops.op_fs_write(target, out); cb(); } catch (err) { cb(err); }
    },
  });
}

const promisify = (fn) => (...args) =>
  new Promise((resolve, reject) => {
    queueMicrotask(() => {
      try { resolve(fn(...args)); } catch (err) { reject(err); }
    });
  });

const promises = {
  readFile: readFileWithEncoding,
  writeFile: writeFileWithEncoding,
  stat: (p) => statAsyncRaw(pathStr(p), true).then(makeStats),
  lstat: (p) => statAsyncRaw(pathStr(p), false).then(makeStats),
  readdir: (p, opts) => readdirAsyncRaw(pathStr(p)).then((entries) => {
    if (opts && opts.withFileTypes) {
      return entries.map((e) => ({
        name: e.name,
        isFile: () => !e.is_dir && !e.is_symlink,
        isDirectory: () => e.is_dir,
        isSymbolicLink: () => e.is_symlink,
      }));
    }
    return entries.map((e) => e.name);
  }),
  mkdir: (p, opts) => mkdirAsync(
    pathStr(p),
    typeof opts === "object" && opts ? Boolean(opts.recursive) : false,
  ),
  rm: (p, opts) => rmAsync(
    pathStr(p),
    typeof opts === "object" && opts ? Boolean(opts.recursive) : false,
  ),
  unlink: (p) => rmAsync(pathStr(p), false),
  copyFile: (src, dst) => copyFileAsync(pathStr(src), pathStr(dst)),
  readlink: promisify(readlinkSync),
  realpath: (p) => realpathAsync(pathStr(p)),
  rename: (src, dst) => renameAsync(pathStr(src), pathStr(dst)),
  appendFile: appendFileWithEncoding,
  chmod: (p, mode) => new Promise((resolve, reject) => {
    queueMicrotask(() => { try { chmodSync(p, mode); resolve(); } catch (e) { reject(e); } });
  }),
  symlink: (t, l) => new Promise((resolve, reject) => {
    queueMicrotask(() => { try { symlinkSync(t, l); resolve(); } catch (e) { reject(e); } });
  }),
  link: (e, n) => new Promise((resolve, reject) => {
    queueMicrotask(() => { try { linkSync(e, n); resolve(); } catch (err) { reject(err); } });
  }),
  truncate: (p, len) => new Promise((resolve, reject) => {
    queueMicrotask(() => { try { truncateSync(p, len); resolve(); } catch (e) { reject(e); } });
  }),
  utimes: (p, a, m) => new Promise((resolve, reject) => {
    queueMicrotask(() => { try { utimesSync(p, a, m); resolve(); } catch (e) { reject(e); } });
  }),
  access: (p) => statAsyncRaw(pathStr(p), true).then(() => undefined, (err) => {
    if (err && err.code) throw err;
    const e = new Error(`ENOENT: ${pathStr(p)}`); e.code = "ENOENT"; throw e;
  }),
};

const constants = {
  F_OK: 0, R_OK: 4, W_OK: 2, X_OK: 1,
  O_RDONLY: 0, O_WRONLY: 1, O_RDWR: 2,
  O_CREAT: 64, O_EXCL: 128, O_TRUNC: 512, O_APPEND: 1024,
  S_IFMT: 0o170000, S_IFREG: 0o100000, S_IFDIR: 0o040000, S_IFLNK: 0o120000,
};

const _ = path;

function callback(syncFn) {
  return (...args) => {
    const cb = args.pop();
    if (typeof cb !== "function") {
      throw new TypeError("callback must be a function");
    }
    try {
      const value = syncFn(...args);
      queueMicrotask(() => cb(null, value));
    } catch (err) {
      queueMicrotask(() => cb(err));
    }
  };
}

const realpath = callback(realpathSync);
realpath.native = realpath;
const stat = callback(statSync);
const lstat = callback(lstatSync);
const readdir = callback(readdirSync);
const readFile = callback(readFileSync);
const writeFile = callback(writeFileSync);
const rename = callback(renameSync);
const appendFile = callback(appendFileSync);
const access = callback((p) => {
  if (!existsSync(p)) {
    const err = new Error(`ENOENT: no such file or directory, access '${p}'`);
    err.code = "ENOENT";
    throw err;
  }
});
const exists = (p, cb) => {
  if (typeof cb === "function") queueMicrotask(() => cb(existsSync(p)));
};

module.exports = {
  readFileSync, writeFileSync, existsSync, statSync, lstatSync, realpathSync,
  readdirSync, mkdirSync, rmSync, unlinkSync, copyFileSync, readlinkSync,
  renameSync, appendFileSync,
  chmodSync, symlinkSync, linkSync, truncateSync, utimesSync,
  chmod: callback(chmodSync),
  symlink: callback(symlinkSync),
  link: callback(linkSync),
  truncate: callback(truncateSync),
  utimes: callback(utimesSync),
  createReadStream, createWriteStream,
  realpath, stat, lstat, readdir, readFile, writeFile, access, exists,
  rename, appendFile,
  watch: () => notAvailable("fs.watch"),
  watchFile: () => notAvailable("fs.watchFile"),
  promises,
  constants,
  Stats: function Stats() {},
};
