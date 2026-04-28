// node:string_decoder - backed by TextDecoder.

(function () {
  class StringDecoder {
    constructor(encoding) {
      this._enc = (encoding || "utf8").toLowerCase().replace(/-/g, "");
      this._dec = new TextDecoder(this._normalize(this._enc), { fatal: false });
    }
    _normalize(e) {
      if (e === "utf8" || e === "utf") return "utf-8";
      if (e === "utf16le" || e === "ucs2" || e === "ucs2le") return "utf-16le";
      return e;
    }
    write(buf) {
      const view = buf instanceof Uint8Array ? buf : new Uint8Array(buf);
      return this._dec.decode(view, { stream: true });
    }
    end(buf) {
      let out = "";
      if (buf) out += this.write(buf);
      out += this._dec.decode();
      return out;
    }
  }
  module.exports = { StringDecoder };
})();
