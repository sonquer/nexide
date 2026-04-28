"use strict";

// node:https — thin wrapper that forwards to the `node:http` client
// pipeline with a default `https:` protocol so the same
// `op_http_request` host op handles plain and TLS-secured requests
// (reqwest auto-selects based on the URL scheme).
//
// `Server` / `createServer` for inbound HTTPS is not supported:
// production deployments terminate TLS at the Rust shield.
// Calling `https.createServer` therefore throws an explicit
// `ERR_NOT_AVAILABLE` error so the failure surfaces during boot
// instead of silently returning an inert object.

const http = require("node:http");

function request(opts, callback) {
  return http._clientRequest("https:", opts, callback);
}

function get(opts, callback) {
  return http._clientGet("https:", opts, callback);
}

function createServer() {
  const err = new Error(
    "https.createServer is not available in nexide; terminate TLS at the Rust shield",
  );
  err.code = "ERR_NOT_AVAILABLE";
  throw err;
}

function Agent() {}
Agent.prototype.destroy = function destroy() {};

const https = {
  request,
  get,
  createServer,
  Agent,
  globalAgent: new Agent(),
};
https.default = https;

module.exports = https;
