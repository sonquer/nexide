// node:tty — non-TTY stub. nexide is a server runtime; stdin/stdout
// are never real TTYs from the application's perspective.

(function () {
  function isatty() {
    return false;
  }

  class ReadStream {
    constructor() { this.isTTY = false; }
  }
  class WriteStream {
    constructor() { this.isTTY = false; this.columns = 80; this.rows = 24; }
    getColorDepth() { return 1; }
    hasColors() { return false; }
    getWindowSize() { return [this.columns, this.rows]; }
  }

  module.exports = { isatty, ReadStream, WriteStream };
})();
