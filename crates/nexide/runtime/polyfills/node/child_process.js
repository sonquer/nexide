"use strict";

// node:child_process - host-backed implementation that wraps
// `tokio::process::Command` through `op_proc_*`. Supports the
// asynchronous shapes Next.js relies on: `spawn`, `exec`,
// `execFile`. The synchronous variants (`spawnSync`,
// `execSync`, `execFileSync`) are not provided because blocking
// the V8 thread would deadlock the worker's event loop; callers
// receive an explicit `ERR_NOT_AVAILABLE` error pointing at the
// async equivalent.

const EventEmitter = require("node:events");
const { Readable, Writable } = require("node:stream");

const ops = Nexide.core.ops;

const SIGNAL_TABLE = {
  SIGHUP: 1,
  SIGINT: 2,
  SIGQUIT: 3,
  SIGKILL: 9,
  SIGUSR1: 10,
  SIGUSR2: 12,
  SIGTERM: 15,
};

function resolveSignal(signal) {
  if (signal === undefined || signal === null) return SIGNAL_TABLE.SIGTERM;
  if (typeof signal === "number") return signal;
  const upper = String(signal).toUpperCase();
  return SIGNAL_TABLE[upper] ?? SIGNAL_TABLE.SIGTERM;
}

function normaliseStdio(input) {
  if (typeof input === "string") return [input, input, input];
  if (Array.isArray(input)) {
    return [input[0] || "pipe", input[1] || "pipe", input[2] || "pipe"]
      .map((m) => (m === "inherit" || m === "ignore" ? m : "pipe"));
  }
  return ["pipe", "pipe", "pipe"];
}

class ChildStdin extends Writable {
  constructor(child) {
    super();
    this._child = child;
    this._closed = false;
  }
  _write(chunk, encoding, callback) {
    if (this._closed) {
      callback(new Error("stdin is closed"));
      return;
    }
    const buf = chunk instanceof Uint8Array
      ? chunk
      : new TextEncoder().encode(String(chunk));
    ops.op_proc_stdin_write(this._child._id, buf).then(
      () => callback(),
      (err) => callback(err),
    );
  }
  end(chunk, encoding, callback) {
    super.end(chunk, encoding, () => {
      this._closed = true;
      ops.op_proc_stdin_close(this._child._id);
      if (typeof callback === "function") callback();
    });
  }
}

class ChildPipe extends Readable {
  constructor(child, isStderr) {
    super();
    this._child = child;
    this._isStderr = isStderr;
    this._closed = false;
    queueMicrotask(() => this._pump());
  }
  _read() { /* pump-driven */ }
  async _pump() {
    const op = this._isStderr ? ops.op_proc_stderr_read : ops.op_proc_stdout_read;
    while (!this._closed) {
      try {
        const chunk = await op(this._child._id, 64 * 1024);
        if (chunk === null) {
          this._closed = true;
          this.push(null);
          return;
        }
        this.push(chunk);
      } catch (err) {
        this._closed = true;
        this.emit("error", err);
        return;
      }
    }
  }
}

class ChildProcess extends EventEmitter {
  constructor(descriptor) {
    super();
    this._id = descriptor.id;
    this.pid = descriptor.pid;
    this.killed = false;
    this.exitCode = null;
    this.signalCode = null;
    this.stdin = descriptor.hasStdin ? new ChildStdin(this) : null;
    this.stdout = descriptor.hasStdout ? new ChildPipe(this, false) : null;
    this.stderr = descriptor.hasStderr ? new ChildPipe(this, true) : null;
    queueMicrotask(() => this._wait());
  }

  async _wait() {
    try {
      const { code, signal } = await ops.op_proc_wait(this._id);
      this.exitCode = code;
      this.signalCode = signal;
      this.emit("exit", code, signal);
      this.emit("close", code, signal);
    } catch (err) {
      this.emit("error", err);
    } finally {
      ops.op_proc_close(this._id);
    }
  }

  kill(signal) {
    try {
      ops.op_proc_kill(this._id, resolveSignal(signal));
      this.killed = true;
      return true;
    } catch (err) {
      this.emit("error", err);
      return false;
    }
  }

  ref() { return this; }
  unref() { return this; }
  disconnect() { /* no IPC channel */ }
}

function spawn(command, args = [], options = {}) {
  const stdio = normaliseStdio(options.stdio);
  const env = options.env ? { ...options.env } : null;
  const descriptor = ops.op_proc_spawn({
    command: String(command),
    args: (args || []).map(String),
    cwd: options.cwd ? String(options.cwd) : null,
    env,
    clearEnv: Boolean(options.env && !options.envInherit),
    stdio,
  });
  return new ChildProcess(descriptor);
}

function exec(command, options, callback) {
  if (typeof options === "function") {
    callback = options;
    options = {};
  }
  options = options || {};
  const shell = options.shell || (process.platform === "win32" ? "cmd.exe" : "/bin/sh");
  const shellArgs = process.platform === "win32" ? ["/d", "/s", "/c", command] : ["-c", command];
  return execFile(shell, shellArgs, options, callback);
}

function execFile(file, args, options, callback) {
  if (typeof args === "function") {
    callback = args;
    args = [];
    options = {};
  } else if (typeof options === "function") {
    callback = options;
    options = {};
  }
  args = args || [];
  options = options || {};
  const child = spawn(file, args, options);
  const stdoutChunks = [];
  const stderrChunks = [];
  if (child.stdout) child.stdout.on("data", (c) => stdoutChunks.push(c));
  if (child.stderr) child.stderr.on("data", (c) => stderrChunks.push(c));
  child.on("error", (err) => {
    if (typeof callback === "function") callback(err, null, null);
  });
  child.on("close", (code) => {
    if (typeof callback !== "function") return;
    const stdout = Buffer.concat(stdoutChunks).toString(options.encoding || "utf8");
    const stderr = Buffer.concat(stderrChunks).toString(options.encoding || "utf8");
    if (code !== 0) {
      const err = new Error(`Command failed with code ${code}: ${file} ${args.join(" ")}`);
      err.code = code;
      callback(err, stdout, stderr);
    } else {
      callback(null, stdout, stderr);
    }
  });
  return child;
}

function unsupportedSync(name) {
  const err = new Error(
    `${name} is not available in nexide; use the async equivalent (would block the event loop)`,
  );
  err.code = "ERR_NOT_AVAILABLE";
  return err;
}

function spawnSync() { throw unsupportedSync("spawnSync"); }
function execSync() { throw unsupportedSync("execSync"); }
function execFileSync() { throw unsupportedSync("execFileSync"); }

function fork() {
  const err = new Error("child_process.fork is not available in nexide; use Worker threads or HTTP for IPC");
  err.code = "ERR_NOT_AVAILABLE";
  throw err;
}

const child_process = {
  spawn,
  exec,
  execFile,
  fork,
  spawnSync,
  execSync,
  execFileSync,
  ChildProcess,
};
child_process.default = child_process;

module.exports = child_process;
