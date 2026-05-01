"use strict";

// node:events - minimal but complete EventEmitter implementation.
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

// Implemented as a function constructor (not an ES2015 `class`) so that
// downstream code can call `EventEmitter.call(this)` from a subclass -
// Node.js's real EventEmitter has the same shape, and bundles such as
// Next.js's vendored `image-size` rely on that ES5 inheritance pattern.
function EventEmitter() {
  if (!this._events || this._events === Object.getPrototypeOf(this)._events) {
    this._events = Object.create(null);
    this._eventsCount = 0;
  }
  this._maxListeners = this._maxListeners ?? undefined;
}

EventEmitter.prototype._events = undefined;
EventEmitter.prototype._eventsCount = 0;
EventEmitter.prototype._maxListeners = undefined;

EventEmitter.prototype.setMaxListeners = function setMaxListeners(n) {
  if (typeof n !== "number" || n < 0 || Number.isNaN(n)) {
    throw new RangeError('The value of "n" must be a non-negative number');
  }
  this._maxListeners = n;
  return this;
};

EventEmitter.prototype.getMaxListeners = function getMaxListeners() {
  return this._maxListeners === undefined
    ? EventEmitter.defaultMaxListeners
    : this._maxListeners;
};

EventEmitter.prototype.emit = function emit(type, ...args) {
  const events = this._events;
  const handlers = events && events[type];
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
      queueMicrotask(() => {
        throw err;
      });
    }
  }
  return true;
};

EventEmitter.prototype._addListener = function _addListener(type, listener, prepend) {
  checkListener(listener);
  if (!this._events || this._events === Object.getPrototypeOf(this)._events) {
    this._events = Object.create(null);
    this._eventsCount = 0;
  }
  // Node fires `newListener` BEFORE the listener is registered so the
  // handler observes the pre-add state. We unwrap `once` wrappers so
  // the user-visible function is reported, matching upstream behavior
  // (see lib/events.js `emit('newListener', ...)`).
  if (this._events.newListener !== undefined) {
    this.emit("newListener", type, listener.listener ? listener.listener : listener);
    if (!this._events) {
      this._events = Object.create(null);
      this._eventsCount = 0;
    }
  }
  const existing = this._events[type];
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
};

EventEmitter.prototype.on = function on(type, listener) {
  return this._addListener(type, listener, false);
};
EventEmitter.prototype.addListener = function addListener(type, listener) {
  return this.on(type, listener);
};
EventEmitter.prototype.prependListener = function prependListener(type, listener) {
  return this._addListener(type, listener, true);
};

EventEmitter.prototype.once = function once(type, listener) {
  checkListener(listener);
  const self = this;
  function wrapper(...args) {
    self.removeListener(type, wrapper);
    listener.apply(self, args);
  }
  wrapper.listener = listener;
  return this.on(type, wrapper);
};

EventEmitter.prototype.prependOnceListener = function prependOnceListener(type, listener) {
  checkListener(listener);
  const self = this;
  function wrapper(...args) {
    self.removeListener(type, wrapper);
    listener.apply(self, args);
  }
  wrapper.listener = listener;
  return this.prependListener(type, wrapper);
};

EventEmitter.prototype.removeListener = function removeListener(type, listener) {
  checkListener(listener);
  const events = this._events;
  if (!events) return this;
  const existing = events[type];
  if (!existing) return this;
  let removed = null;
  if (existing === listener || existing.listener === listener) {
    removed = existing.listener || existing;
    delete events[type];
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
    removed = existing[position].listener || existing[position];
    if (existing.length === 1) {
      delete events[type];
      this._eventsCount--;
    } else {
      existing.splice(position, 1);
    }
  }
  if (removed && events.removeListener !== undefined) {
    this.emit("removeListener", type, removed);
  }
  return this;
};
EventEmitter.prototype.off = function off(type, listener) {
  return this.removeListener(type, listener);
};

EventEmitter.prototype.removeAllListeners = function removeAllListeners(type) {
  if (type === undefined) {
    this._events = Object.create(null);
    this._eventsCount = 0;
    return this;
  }
  if (this._events && this._events[type]) {
    delete this._events[type];
    this._eventsCount--;
  }
  return this;
};

EventEmitter.prototype.listeners = function listeners(type) {
  const existing = this._events && this._events[type];
  if (!existing) return [];
  return (Array.isArray(existing) ? existing : [existing]).map(
    (fn) => fn.listener || fn,
  );
};

EventEmitter.prototype.rawListeners = function rawListeners(type) {
  const existing = this._events && this._events[type];
  if (!existing) return [];
  return Array.isArray(existing) ? existing.slice() : [existing];
};

EventEmitter.prototype.listenerCount = function listenerCount(type) {
  const existing = this._events && this._events[type];
  if (!existing) return 0;
  return Array.isArray(existing) ? existing.length : 1;
};

EventEmitter.prototype.eventNames = function eventNames() {
  return this._events ? Reflect.ownKeys(this._events) : [];
};

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
