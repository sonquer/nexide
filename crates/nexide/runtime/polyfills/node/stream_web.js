"use strict";

// node:stream/web - re-exports the WHATWG Streams classes that nexide
// installs on globalThis from the V8/web_apis bootstrap.
//
// This is the surface Next.js (and modern fetch-based deps such as
// undici, node-fetch@3, formdata-polyfill) hits when they `require
// ("node:stream/web")` in standalone bundles.

function pick(name) {
  const v = globalThis[name];
  if (typeof v === "undefined") {
    return function MissingWebStream() {
      throw new Error(
        `nexide: ${name} is not available in this isolate (node:stream/web)`,
      );
    };
  }
  return v;
}

module.exports = {
  ReadableStream: pick("ReadableStream"),
  ReadableStreamDefaultReader: pick("ReadableStreamDefaultReader"),
  ReadableStreamBYOBReader: pick("ReadableStreamBYOBReader"),
  ReadableStreamDefaultController: pick("ReadableStreamDefaultController"),
  ReadableByteStreamController: pick("ReadableByteStreamController"),
  WritableStream: pick("WritableStream"),
  WritableStreamDefaultWriter: pick("WritableStreamDefaultWriter"),
  WritableStreamDefaultController: pick("WritableStreamDefaultController"),
  TransformStream: pick("TransformStream"),
  TransformStreamDefaultController: pick("TransformStreamDefaultController"),
  ByteLengthQueuingStrategy: pick("ByteLengthQueuingStrategy"),
  CountQueuingStrategy: pick("CountQueuingStrategy"),
  TextEncoderStream: pick("TextEncoderStream"),
  TextDecoderStream: pick("TextDecoderStream"),
  CompressionStream: pick("CompressionStream"),
  DecompressionStream: pick("DecompressionStream"),
};
