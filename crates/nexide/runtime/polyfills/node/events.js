"use strict";

// node:events — minimal but complete EventEmitter implementation.
// Compatible with the Node.js public API used by Next.js standalone
// and most middleware libraries.

const kCapture = Symbol.for("nodejs.rejection");

function checkListener(listener) {
  if (typeof listener !== "function") {
    throw new TypeError(
      'The "listener" argument must be of type Function. Received type ' +
        typeof listener,
    );
  }
}

class EventEmitter {
  constructor() {
    this._events = Object.create(null);
    this._eventsCount = 0;
    this._maxListeners = undefined;
  }

  setMaxListeners(n) {
    if (typeof n !== "number" || n < 0 || Number.isNaN(n)) {
      throw new RangeError(
        'The value of "n" must be a non-negative number',
      );
    }
    this._maxListeners = n;
    return this;
  }

  getMaxListeners() {
    return this._maxListeners === undefined
      ? EventEmitter.defaultMaxListeners
      : this._maxListeners;
  }

  emit(type, ...args) {
    const handlers = this._events[type];
    if (!handlers) {
      if (type === "error") {
        const err = args[0];
        throw err instanceof Error ? err : new Error("Unhandled error");
      }
      return false;
    }
    const list = Array.isArray(handlers) ? handlers.slice() : [handlers];
    for (const fn of list) {
      try {
        fn.apply(this, args);
      } catch (err) {
        queueMicrotask(() => { throw err; });
      }
    }
    return true;
  }

  _addListener(type, listener, prepend) {
    checkListener(listener);
    let existing = this._events[type];
    if (!existing) {
      this._events[type] = listener;
      this._eventsCount++;
    } else if (typeof existing === "function") {
      this._events[type] = prepend ? [listener, existing] : [existing, listener];
    } else if (prepend) {
      existing.unshift(listener);
    } else {
      existing.push(listener);
    }
    return this;
  }

  on(type, listener) { return this._addListener(type, listener, false); }
  addListener(type, listener) { return this.on(type, listener); }
  prependListener(type, listener) { return this._addListener(type, listener, true); }

  once(type, listener) {
    checkListener(listener);
    const wrapper = (...args) => {
      this.removeListener(type, wrapper);
      listener.apply(this, args);
    };
    wrapper.listener = listener;
    return this.on(type, wrapper);
  }

  prependOnceListener(type, listener) {
    checkListener(listener);
    const wrapper = (...args) => {
      this.removeListener(type, wrapper);
      listener.apply(this, args);
    };
    wrapper.listener = listener;
    return this.prependListener(type, wrapper);
  }

  removeListener(type, listener) {
    checkListener(listener);
    const existing = this._events[type];
    if (!existing) return this;
    if (existing === listener || existing.listener === listener) {
      delete this._events[type];
      this._eventsCount--;
    } else if (Array.isArray(existing)) {
      let position = -1;
      for (let i = existing.length - 1; i >= 0; i--) {
        if (existing[i] === listener || existing[i].listener === listener) {
          position = i;
          break;
        }
      }
      if (position < 0) return this;
      if (existing.length === 1) {
        delete this._events[type];
        this._eventsCount--;
      } else {
        existing.splice(position, 1);
      }
    }
    return this;
  }
  off(type, listener) { return this.removeListener(type, listener); }

  removeAllListeners(type) {
    if (type === undefined) {
      this._events = Object.create(null);
      this._eventsCount = 0;
      return this;
    }
    if (this._events[type]) {
      delete this._events[type];
      this._eventsCount--;
    }
    return this;
  }

  listeners(type) {
    const existing = this._events[type];
    if (!existing) return [];
    return (Array.isArray(existing) ? existing : [existing]).map(
      (fn) => fn.listener || fn,
    );
  }

  rawListeners(type) {
    const existing = this._events[type];
    if (!existing) return [];
    return Array.isArray(existing) ? existing.slice() : [existing];
  }

  listenerCount(type) {
    const existing = this._events[type];
    if (!existing) return 0;
    return Array.isArray(existing) ? existing.length : 1;
  }

  eventNames() {
    return Reflect.ownKeys(this._events);
  }
}

EventEmitter.defaultMaxListeners = 10;
EventEmitter.captureRejectionSymbol = kCapture;
EventEmitter.EventEmitter = EventEmitter;

EventEmitter.once = function (emitter, name) {
  return new Promise((resolve, reject) => {
    const onEvent = (...args) => {
      emitter.removeListener("error", onError);
      resolve(args);
    };
    const onError = (err) => {
      emitter.removeListener(name, onEvent);
      reject(err);
    };
    emitter.once(name, onEvent);
    emitter.once("error", onError);
  });
};

EventEmitter.on = function (emitter, name) {
  const queue = [];
  const waiters = [];
  emitter.on(name, (...args) => {
    if (waiters.length) waiters.shift().resolve({ value: args, done: false });
    else queue.push(args);
  });
  return {
    [Symbol.asyncIterator]() { return this; },
    next() {
      if (queue.length) {
        return Promise.resolve({ value: queue.shift(), done: false });
      }
      return new Promise((resolve, reject) => waiters.push({ resolve, reject }));
    },
    return() { return Promise.resolve({ value: undefined, done: true }); },
  };
};

module.exports = EventEmitter;
module.exports.EventEmitter = EventEmitter;
module.exports.default = EventEmitter;
