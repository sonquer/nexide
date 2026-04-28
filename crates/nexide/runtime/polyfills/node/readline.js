"use strict";

// node:readline - functional line-buffering reader over any Node-style
// Readable (or anything emitting `data`/`end` events). Implements the
// surface most non-TTY deps actually use:
//
//  - createInterface({ input, output, prompt, crlfDelay })
//  - 'line' events with newline stripped (handles \n, \r\n, lone \r)
//  - 'close' / 'pause' / 'resume'
//  - prompt() / setPrompt() / write()
//  - question(query, [opts], cb)
//  - async iterator yielding lines until input ends or close() is called
//
// Out of scope: TTY raw mode, history, tab completion, keypress UI -
// these require the controlling process to own a real terminal which
// nexide workloads (server-side Next.js) don't have.

const EventEmitter = require("node:events");
const { StringDecoder } = require("node:string_decoder");

const kInput = Symbol("input");
const kOutput = Symbol("output");
const kClosed = Symbol("closed");
const kPaused = Symbol("paused");
const kBuffer = Symbol("buffer");
const kDecoder = Symbol("decoder");
const kPrompt = Symbol("prompt");
const kPendingResolvers = Symbol("pendingResolvers");
const kIterQueue = Symbol("iterQueue");
const kIterPullers = Symbol("iterPullers");

function isReadable(input) {
  return input && typeof input.on === "function";
}

function pushChunk(rl, chunk) {
  if (rl[kClosed]) return;
  if (chunk == null) return;
  let text;
  if (typeof chunk === "string") {
    text = chunk;
  } else if (chunk instanceof Uint8Array) {
    text = rl[kDecoder].write(chunk);
  } else if (typeof chunk === "object" && typeof chunk.toString === "function") {
    text = String(chunk);
  } else {
    return;
  }
  rl[kBuffer] += text;
  drainLines(rl);
}

function drainLines(rl) {
  // Consume every fully-terminated line currently in the buffer.
  // Handles "\n", "\r\n" and lone "\r" (old-Mac style); the trailing
  // partial line stays in the buffer for the next chunk.
  let idx;
  // eslint-disable-next-line no-cond-assign
  while ((idx = rl[kBuffer].search(/\r\n|\r|\n/)) !== -1) {
    const match = rl[kBuffer].match(/\r\n|\r|\n/);
    const sepLen = match ? match[0].length : 1;
    const line = rl[kBuffer].slice(0, idx);
    rl[kBuffer] = rl[kBuffer].slice(idx + sepLen);
    deliverLine(rl, line);
    if (rl[kClosed]) return;
  }
}

function deliverLine(rl, line) {
  // Pending question() resolvers win first (Node's behaviour).
  const pending = rl[kPendingResolvers].shift();
  if (pending) {
    pending(line);
    return;
  }
  // Async iterator pullers next.
  const puller = rl[kIterPullers].shift();
  if (puller) {
    puller({ value: line, done: false });
  } else {
    rl[kIterQueue].push(line);
  }
  rl.emit("line", line);
}

function flushBufferOnEnd(rl) {
  if (rl[kBuffer].length > 0) {
    const tail = rl[kBuffer];
    rl[kBuffer] = "";
    deliverLine(rl, tail);
  }
}

class Interface extends EventEmitter {
  constructor(opts = {}) {
    super();
    this[kInput] = opts.input;
    this[kOutput] = opts.output;
    this[kClosed] = false;
    this[kPaused] = false;
    this[kBuffer] = "";
    this[kDecoder] = new StringDecoder("utf8");
    this[kPrompt] = typeof opts.prompt === "string" ? opts.prompt : "> ";
    this[kPendingResolvers] = [];
    this[kIterQueue] = [];
    this[kIterPullers] = [];
    this.terminal = !!opts.terminal;

    if (isReadable(this[kInput])) {
      const onData = (chunk) => pushChunk(this, chunk);
      const onEnd = () => {
        flushBufferOnEnd(this);
        this.close();
      };
      const onError = (err) => this.emit("error", err);
      this[kInput].on("data", onData);
      this[kInput].on("end", onEnd);
      this[kInput].on("close", onEnd);
      this[kInput].on("error", onError);
      this._removeInputListeners = () => {
        try {
          this[kInput].removeListener("data", onData);
          this[kInput].removeListener("end", onEnd);
          this[kInput].removeListener("close", onEnd);
          this[kInput].removeListener("error", onError);
        } catch { /* noop */ }
      };
    } else {
      this._removeInputListeners = () => {};
    }
  }

  get input() { return this[kInput]; }
  get output() { return this[kOutput]; }
  get closed() { return this[kClosed]; }

  setPrompt(prompt) { this[kPrompt] = String(prompt); }
  getPrompt() { return this[kPrompt]; }

  prompt(_preserveCursor) {
    if (this[kClosed]) return;
    this.write(this[kPrompt]);
  }

  write(data, _key) {
    if (this[kClosed]) return;
    const out = this[kOutput];
    if (out && typeof out.write === "function" && data != null) {
      try { out.write(String(data)); } catch { /* noop */ }
    }
  }

  question(query, optsOrCb, maybeCb) {
    const cb = typeof optsOrCb === "function" ? optsOrCb : maybeCb;
    if (typeof cb !== "function") {
      throw new TypeError("readline.question requires a callback");
    }
    if (this[kClosed]) {
      cb("");
      return;
    }
    this.write(query);
    this[kPendingResolvers].push((line) => cb(line));
  }

  pause() {
    this[kPaused] = true;
    if (this[kInput] && typeof this[kInput].pause === "function") {
      try { this[kInput].pause(); } catch { /* noop */ }
    }
    return this;
  }

  resume() {
    this[kPaused] = false;
    if (this[kInput] && typeof this[kInput].resume === "function") {
      try { this[kInput].resume(); } catch { /* noop */ }
    }
    return this;
  }

  close() {
    if (this[kClosed]) return;
    this[kClosed] = true;
    try { this._removeInputListeners(); } catch { /* noop */ }
    // Drain any pending question() resolvers with empty string (Node
    // behaviour: resolves callers when stream ends mid-question).
    while (this[kPendingResolvers].length > 0) {
      const r = this[kPendingResolvers].shift();
      try { r(""); } catch { /* noop */ }
    }
    // Notify async iterator pullers that the stream is over.
    while (this[kIterPullers].length > 0) {
      const puller = this[kIterPullers].shift();
      puller({ value: undefined, done: true });
    }
    this.emit("close");
  }

  [Symbol.asyncIterator]() {
    const rl = this;
    return {
      next() {
        if (rl[kIterQueue].length > 0) {
          return Promise.resolve({ value: rl[kIterQueue].shift(), done: false });
        }
        if (rl[kClosed]) {
          return Promise.resolve({ value: undefined, done: true });
        }
        return new Promise((resolve) => {
          rl[kIterPullers].push(resolve);
        });
      },
      return() {
        rl.close();
        return Promise.resolve({ value: undefined, done: true });
      },
      [Symbol.asyncIterator]() { return this; },
    };
  }
}

function createInterface(input, output, completer, terminal) {
  // Node accepts both shapes: createInterface(opts) and
  // createInterface(input, output?, completer?, terminal?).
  const opts = (input && typeof input === "object" && !isReadable(input)
                && !(input instanceof Uint8Array))
    ? input
    : { input, output, completer, terminal };
  return new Interface(opts);
}

function clearLine(_stream, _dir, cb) { if (cb) cb(); return true; }
function clearScreenDown(_stream, cb) { if (cb) cb(); return true; }
function cursorTo(_stream, _x, _y, cb) { if (cb) cb(); return true; }
function moveCursor(_stream, _dx, _dy, cb) { if (cb) cb(); return true; }
function emitKeypressEvents() { /* no-op: nexide workloads are non-TTY */ }

module.exports = {
  Interface,
  createInterface,
  clearLine,
  clearScreenDown,
  cursorTo,
  moveCursor,
  emitKeypressEvents,
};
