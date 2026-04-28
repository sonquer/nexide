"use strict";

// node:diagnostics_channel - lightweight pub/sub for instrumentation,
// modelled on Node's stable surface (channel, subscribe, unsubscribe,
// hasSubscribers, tracingChannel). Used by undici, OpenTelemetry SDKs,
// Sentry/Datadog APMs.

const channels = new Map();

class Channel {
  constructor(name) {
    this.name = name;
    this._subs = new Set();
  }
  get hasSubscribers() {
    return this._subs.size > 0;
  }
  subscribe(fn) {
    if (typeof fn !== "function") {
      throw new TypeError("diagnostics_channel: subscriber must be a function");
    }
    this._subs.add(fn);
  }
  unsubscribe(fn) {
    return this._subs.delete(fn);
  }
  publish(message) {
    if (this._subs.size === 0) return;
    for (const fn of [...this._subs]) {
      try { fn(message, this.name); } catch { /* swallow per Node */ }
    }
  }
  bindStore() { /* no-op: ALS integration not modelled */ }
  unbindStore() { /* no-op */ }
  runStores(_data, fn, thisArg, ...args) {
    return fn.apply(thisArg, args);
  }
}

function channel(name) {
  let ch = channels.get(name);
  if (!ch) {
    ch = new Channel(name);
    channels.set(name, ch);
  }
  return ch;
}

function hasSubscribers(name) {
  const ch = channels.get(name);
  return !!ch && ch.hasSubscribers;
}

function subscribe(name, fn) {
  channel(name).subscribe(fn);
}

function unsubscribe(name, fn) {
  const ch = channels.get(name);
  return ch ? ch.unsubscribe(fn) : false;
}

class TracingChannel {
  constructor(nameOrChannels) {
    const base = typeof nameOrChannels === "string"
      ? nameOrChannels
      : null;
    if (base) {
      this.start = channel(`tracing:${base}:start`);
      this.end = channel(`tracing:${base}:end`);
      this.asyncStart = channel(`tracing:${base}:asyncStart`);
      this.asyncEnd = channel(`tracing:${base}:asyncEnd`);
      this.error = channel(`tracing:${base}:error`);
    } else if (nameOrChannels && typeof nameOrChannels === "object") {
      this.start = nameOrChannels.start;
      this.end = nameOrChannels.end;
      this.asyncStart = nameOrChannels.asyncStart;
      this.asyncEnd = nameOrChannels.asyncEnd;
      this.error = nameOrChannels.error;
    }
  }
  get hasSubscribers() {
    return [this.start, this.end, this.asyncStart, this.asyncEnd, this.error]
      .some((c) => c && c.hasSubscribers);
  }
  subscribe(handlers) {
    for (const k of ["start", "end", "asyncStart", "asyncEnd", "error"]) {
      if (handlers[k] && this[k]) this[k].subscribe(handlers[k]);
    }
  }
  unsubscribe(handlers) {
    let ok = true;
    for (const k of ["start", "end", "asyncStart", "asyncEnd", "error"]) {
      if (handlers[k] && this[k]) {
        ok = this[k].unsubscribe(handlers[k]) && ok;
      }
    }
    return ok;
  }
  traceSync(fn, ctx, thisArg, ...args) {
    if (this.start) this.start.publish(ctx);
    try {
      const result = fn.apply(thisArg, args);
      if (this.end) this.end.publish(ctx);
      return result;
    } catch (err) {
      if (this.error) this.error.publish({ ...ctx, error: err });
      if (this.end) this.end.publish(ctx);
      throw err;
    }
  }
  tracePromise(fn, ctx, thisArg, ...args) {
    if (this.start) this.start.publish(ctx);
    let p;
    try { p = fn.apply(thisArg, args); }
    catch (err) {
      if (this.error) this.error.publish({ ...ctx, error: err });
      if (this.end) this.end.publish(ctx);
      throw err;
    }
    if (this.end) this.end.publish(ctx);
    return Promise.resolve(p).then(
      (result) => {
        if (this.asyncStart) this.asyncStart.publish({ ...ctx, result });
        if (this.asyncEnd) this.asyncEnd.publish({ ...ctx, result });
        return result;
      },
      (err) => {
        if (this.error) this.error.publish({ ...ctx, error: err });
        if (this.asyncEnd) this.asyncEnd.publish({ ...ctx, error: err });
        throw err;
      },
    );
  }
  traceCallback(fn, position, ctx, thisArg, ...args) {
    if (this.start) this.start.publish(ctx);
    const cb = args[position];
    args[position] = (err, ...rest) => {
      if (err && this.error) this.error.publish({ ...ctx, error: err });
      if (this.asyncStart) this.asyncStart.publish({ ...ctx, error: err, result: rest[0] });
      try { cb(err, ...rest); }
      finally {
        if (this.asyncEnd) this.asyncEnd.publish({ ...ctx, error: err, result: rest[0] });
      }
    };
    try {
      const r = fn.apply(thisArg, args);
      if (this.end) this.end.publish(ctx);
      return r;
    } catch (err) {
      if (this.error) this.error.publish({ ...ctx, error: err });
      if (this.end) this.end.publish(ctx);
      throw err;
    }
  }
}

function tracingChannel(nameOrChannels) {
  return new TracingChannel(nameOrChannels);
}

module.exports = {
  Channel,
  TracingChannel,
  channel,
  hasSubscribers,
  subscribe,
  unsubscribe,
  tracingChannel,
};
