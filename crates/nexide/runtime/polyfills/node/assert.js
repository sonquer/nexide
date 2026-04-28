// node:assert — minimal subset Next.js needs. Throws AssertionError
// with `.code = 'ERR_ASSERTION'` on failure.

(function () {
  class AssertionError extends Error {
    constructor(opts) {
      const message =
        (opts && typeof opts === "object" && opts.message) ||
        "Assertion failed";
      super(message);
      this.name = "AssertionError";
      this.code = "ERR_ASSERTION";
      if (opts && typeof opts === "object") {
        if ("actual" in opts) this.actual = opts.actual;
        if ("expected" in opts) this.expected = opts.expected;
        if ("operator" in opts) this.operator = opts.operator;
      }
    }
  }

  function fail(actual, expected, message, operator) {
    if (arguments.length === 1 && typeof actual === "string") {
      throw new AssertionError({ message: actual });
    }
    throw new AssertionError({
      message: message ?? "Failed",
      actual,
      expected,
      operator: operator ?? "fail",
    });
  }

  function assert(value, message) {
    if (!value) {
      throw new AssertionError({
        message: message ?? "Assertion failed",
        actual: value,
        expected: true,
        operator: "==",
      });
    }
  }

  function ok(value, message) {
    assert(value, message);
  }

  function strictEqual(actual, expected, message) {
    if (!Object.is(actual, expected)) {
      throw new AssertionError({
        message: message ??
          `Expected ${String(expected)} === ${String(actual)}`,
        actual,
        expected,
        operator: "strictEqual",
      });
    }
  }

  function notStrictEqual(actual, expected, message) {
    if (Object.is(actual, expected)) {
      throw new AssertionError({
        message: message ?? "Values are strictly equal",
        actual,
        expected,
        operator: "notStrictEqual",
      });
    }
  }

  function deepStrictEqual(actual, expected, message) {
    if (!deepEqualImpl(actual, expected, true)) {
      throw new AssertionError({
        message: message ?? "Values are not deep-strict-equal",
        actual,
        expected,
        operator: "deepStrictEqual",
      });
    }
  }

  function deepEqual(actual, expected, message) {
    if (!deepEqualImpl(actual, expected, false)) {
      throw new AssertionError({
        message: message ?? "Values are not deep-equal",
        actual,
        expected,
        operator: "deepEqual",
      });
    }
  }

  function deepEqualImpl(a, b, strict) {
    if (strict ? Object.is(a, b) : a == b) return true;
    if (a === null || b === null) return false;
    if (typeof a !== "object" || typeof b !== "object") return false;
    if (Array.isArray(a) !== Array.isArray(b)) return false;
    const keysA = Object.keys(a);
    const keysB = Object.keys(b);
    if (keysA.length !== keysB.length) return false;
    for (const k of keysA) {
      if (!deepEqualImpl(a[k], b[k], strict)) return false;
    }
    return true;
  }

  function throws(fn, expected, message) {
    let thrown;
    try { fn(); } catch (e) { thrown = e; }
    if (!thrown) {
      throw new AssertionError({
        message: message ?? "Missing expected exception",
        operator: "throws",
      });
    }
    if (expected && expected instanceof RegExp) {
      if (!expected.test(String(thrown.message ?? thrown))) {
        throw new AssertionError({
          message: message ?? "Exception did not match RegExp",
          actual: thrown,
          expected,
          operator: "throws",
        });
      }
    }
  }

  function doesNotThrow(fn, message) {
    try { fn(); } catch (e) {
      throw new AssertionError({
        message: message ?? "Got unexpected exception: " + String(e),
        actual: e,
        operator: "doesNotThrow",
      });
    }
  }

  assert.AssertionError = AssertionError;
  assert.fail = fail;
  assert.ok = ok;
  assert.equal = (a, b, m) => {
    if (a != b) throw new AssertionError({ message: m ?? "not ==", actual: a, expected: b });
  };
  assert.notEqual = (a, b, m) => {
    if (a == b) throw new AssertionError({ message: m ?? "is ==", actual: a, expected: b });
  };
  assert.strictEqual = strictEqual;
  assert.notStrictEqual = notStrictEqual;
  assert.deepEqual = deepEqual;
  assert.deepStrictEqual = deepStrictEqual;
  assert.notDeepStrictEqual = (a, b, m) => {
    if (deepEqualImpl(a, b, true)) {
      throw new AssertionError({ message: m ?? "deep-strict-equal", actual: a, expected: b });
    }
  };
  assert.throws = throws;
  assert.doesNotThrow = doesNotThrow;
  assert.strict = assert;

  module.exports = assert;
})();
