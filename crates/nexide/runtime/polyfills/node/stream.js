"use strict";

// node:stream - minimal Readable/Writable/Duplex/Transform/PassThrough
// implementation sufficient for the surface Next.js standalone uses
// (mostly Readable.from + pipeline + finished + simple piping).

const EventEmitter = require("node:events");

class Readable extends EventEmitter {
  constructor(opts = {}) {
    super();
    this._readable = true;
    this._buffer = [];
    this._ended = false;
    this._reading = false;
    this._destroyed = false;
    this._readImpl = opts.read;
    this._encoding = null;
  }
  static from(iterable) {
    const r = new Readable();
    (async () => {
      try {
        for await (const chunk of iterable) {
          r.push(chunk);
        }
        r.push(null);
      } catch (err) {
        r.destroy(err);
      }
    })();
    return r;
  }
  setEncoding(enc) { this._encoding = enc; return this; }
  push(chunk) {
    if (chunk === null) { this._ended = true; this.emit("end"); return false; }
    this._buffer.push(chunk);
    this.emit("data", chunk);
    return true;
  }
  read() {
    if (this._buffer.length === 0) return null;
    return this._buffer.shift();
  }
  pipe(dest) {
    this.on("data", (chunk) => dest.write(chunk));
    this.on("end", () => dest.end());
    this.on("error", (err) => dest.destroy(err));
    return dest;
  }
  destroy(err) {
    if (this._destroyed) return this;
    this._destroyed = true;
    if (err) this.emit("error", err);
    this.emit("close");
    return this;
  }
  [Symbol.asyncIterator]() {
    const self = this;
    return {
      async next() {
        if (self._buffer.length) return { value: self._buffer.shift(), done: false };
        if (self._ended) return { value: undefined, done: true };
        return new Promise((resolve, reject) => {
          const onData = () => {
            if (!self._buffer.length) return;
            cleanup();
            resolve({ value: self._buffer.shift(), done: false });
          };
          const onEnd = () => { cleanup(); resolve({ value: undefined, done: true }); };
          const onErr = (err) => { cleanup(); reject(err); };
          const cleanup = () => {
            self.removeListener("data", onData);
            self.removeListener("end", onEnd);
            self.removeListener("error", onErr);
          };
          self.on("data", onData);
          self.once("end", onEnd);
          self.once("error", onErr);
        });
      },
    };
  }
}

class Writable extends EventEmitter {
  constructor(opts = {}) {
    super();
    this._writable = true;
    this._chunks = [];
    this._ended = false;
    this._writeImpl = opts.write;
    this._finalImpl = opts.final;
  }
  write(chunk, encoding, cb) {
    if (typeof encoding === "function") { cb = encoding; encoding = undefined; }
    if (this._ended) {
      const err = new Error("write after end");
      if (cb) cb(err); else this.emit("error", err);
      return false;
    }
    this._chunks.push(chunk);
    if (this._writeImpl) {
      this._writeImpl(chunk, encoding, cb || (() => {}));
    } else if (cb) {
      cb();
    }
    return true;
  }
  end(chunk, encoding, cb) {
    if (typeof chunk === "function") { cb = chunk; chunk = undefined; }
    if (chunk !== undefined && chunk !== null) this.write(chunk, encoding);
    this._ended = true;
    const finish = () => { this.emit("finish"); if (cb) cb(); };
    if (this._finalImpl) this._finalImpl(finish); else finish();
    return this;
  }
  destroy(err) {
    if (err) this.emit("error", err);
    this.emit("close");
    return this;
  }
}

class Duplex extends Readable {
  constructor(opts = {}) {
    super(opts);
    this._writableInner = new Writable(opts);
  }
  write(chunk, encoding, cb) { return this._writableInner.write(chunk, encoding, cb); }
  end(...args) { return this._writableInner.end(...args); }
}

class Transform extends Duplex {
  constructor(opts = {}) {
    super(opts);
    this._transform = opts.transform;
  }
  write(chunk, encoding, cb) {
    const done = (err, transformed) => {
      if (err) { if (cb) cb(err); return; }
      if (transformed !== undefined) this.push(transformed);
      if (cb) cb();
    };
    if (this._transform) this._transform(chunk, encoding, done);
    else done(null, chunk);
    return true;
  }
}

class PassThrough extends Transform {
  constructor(opts) { super({ ...opts, transform: (c, _e, cb) => cb(null, c) }); }
}

function pipeline(...args) {
  const cb = typeof args[args.length - 1] === "function" ? args.pop() : null;
  const streams = args;
  const promise = new Promise((resolve, reject) => {
    let last = streams[0];
    for (let i = 1; i < streams.length; i++) last = last.pipe(streams[i]);
    last.on("finish", resolve);
    last.on("end", resolve);
    for (const s of streams) s.on("error", reject);
  });
  if (cb) {
    promise.then(() => cb(null), cb);
    return undefined;
  }
  return promise;
}

function finished(stream, cb) {
  const done = (err) => { cleanup(); cb && cb(err || null); };
  const onEnd = () => done();
  const onFinish = () => done();
  const onError = (err) => done(err);
  const cleanup = () => {
    stream.removeListener("end", onEnd);
    stream.removeListener("finish", onFinish);
    stream.removeListener("error", onError);
  };
  stream.once("end", onEnd);
  stream.once("finish", onFinish);
  stream.once("error", onError);
  if (!cb) {
    return new Promise((resolve, reject) => {
      cb = (err) => err ? reject(err) : resolve();
    });
  }
  return undefined;
}

class Stream extends EventEmitter {
  pipe(dest) { this.on("data", (c) => dest.write && dest.write(c)); this.on("end", () => dest.end && dest.end()); return dest; }
}

const stream = Stream;
stream.Stream = Stream;
stream.Readable = Readable;
stream.Writable = Writable;
stream.Duplex = Duplex;
stream.Transform = Transform;
stream.PassThrough = PassThrough;
stream.pipeline = pipeline;
stream.finished = finished;
stream.default = stream;
module.exports = stream;
