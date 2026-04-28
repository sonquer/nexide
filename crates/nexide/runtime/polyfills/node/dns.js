"use strict";

/**
 * Polyfill for Node.js `node:dns`.
 *
 * `dns.lookup` follows Node's behaviour: it consults the OS resolver
 * (going through `/etc/hosts`, NSS modules, mDNS, …) via the
 * host-side `op_dns_lookup` and is therefore the right entry point
 * for application-style "resolve a hostname to one of its IPs".
 *
 * `dns.resolve*` queries hit a recursive resolver directly (hickory,
 * configured from `/etc/resolv.conf`) and so they are the right
 * entry point for typed record queries (MX, TXT, SRV, …).
 *
 * Errors are surfaced as standard `Error` instances with `.code`
 * set to the Node-canonical string (`ENOTFOUND`, `ETIMEOUT`, …) so
 * caller code can pattern-match on `err.code`.
 */

const ops = Nexide.core.ops;

function dnsError(code, message) {
  const err = new Error(message);
  err.code = code;
  return err;
}

function familyOption(opts) {
  if (typeof opts === "number") return opts;
  if (opts && typeof opts.family === "number") return opts.family;
  return 0;
}

/**
 * Node-compatible callback adapter — accepts either `(host, cb)` or
 * `(host, options, cb)`. `options.all` switches between the
 * single-result and array-result shape Node uses.
 */
function lookup(host, options, callback) {
  if (typeof options === "function") {
    callback = options;
    options = {};
  }
  options = options || {};
  const family = familyOption(options);
  const all = !!options.all;
  ops.op_dns_lookup(String(host), family, all).then(
    (res) => {
      if (all) {
        callback(null, res);
      } else {
        callback(null, res.address, res.family);
      }
    },
    (err) => callback(err),
  );
}

function nodeify(promiseFactory) {
  return function (host, callback) {
    promiseFactory(host).then(
      (res) => callback(null, res),
      (err) => callback(err),
    );
  };
}

/** Resolves `host` to its IPv4 (`A`) addresses. */
function resolve4(host) {
  return ops.op_dns_resolve4(String(host));
}

/** Resolves `host` to its IPv6 (`AAAA`) addresses. */
function resolve6(host) {
  return ops.op_dns_resolve6(String(host));
}

/** Resolves `host` to its mail-exchange records. */
function resolveMx(host) {
  return ops.op_dns_resolve_mx(String(host));
}

/** Resolves `host` to its TXT records, each as a `string[]` of chunks. */
function resolveTxt(host) {
  return ops.op_dns_resolve_txt(String(host));
}

/** Resolves `host` to its CNAME records. */
function resolveCname(host) {
  return ops.op_dns_resolve_cname(String(host));
}

/** Resolves `host` to its NS records. */
function resolveNs(host) {
  return ops.op_dns_resolve_ns(String(host));
}

/** Resolves `host` to its SRV records. */
function resolveSrv(host) {
  return ops.op_dns_resolve_srv(String(host));
}

/** PTR lookup — returns the host names registered for `ip`. */
function reverse(ip) {
  return ops.op_dns_reverse(String(ip));
}

/**
 * Dispatches by record type to mirror Node's overloaded
 * `dns.resolve(host[, rrtype], callback)` shape.
 */
function resolve(host, rrtype, callback) {
  if (typeof rrtype === "function") {
    callback = rrtype;
    rrtype = "A";
  }
  const type = String(rrtype || "A").toUpperCase();
  let p;
  switch (type) {
    case "A": p = resolve4(host); break;
    case "AAAA": p = resolve6(host); break;
    case "MX": p = resolveMx(host); break;
    case "TXT": p = resolveTxt(host); break;
    case "CNAME": p = resolveCname(host); break;
    case "NS": p = resolveNs(host); break;
    case "SRV": p = resolveSrv(host); break;
    default:
      queueMicrotask(() =>
        callback(dnsError("ENOTSUP", "rrtype " + type + " not supported"))
      );
      return;
  }
  p.then((r) => callback(null, r), (err) => callback(err));
}

const promises = {
  lookup(host, options) {
    options = options || {};
    const family = familyOption(options);
    const all = !!options.all;
    return ops.op_dns_lookup(String(host), family, all);
  },
  resolve(host, rrtype) {
    return new Promise((resolve, reject) => {
      module.exports.resolve(host, rrtype, (err, res) =>
        err ? reject(err) : resolve(res)
      );
    });
  },
  resolve4: (host) => resolve4(host),
  resolve6: (host) => resolve6(host),
  resolveMx: (host) => resolveMx(host),
  resolveTxt: (host) => resolveTxt(host),
  resolveCname: (host) => resolveCname(host),
  resolveNs: (host) => resolveNs(host),
  resolveSrv: (host) => resolveSrv(host),
  reverse: (ip) => reverse(ip),
};

module.exports = {
  lookup,
  resolve,
  resolve4: nodeify(resolve4),
  resolve6: nodeify(resolve6),
  resolveMx: nodeify(resolveMx),
  resolveTxt: nodeify(resolveTxt),
  resolveCname: nodeify(resolveCname),
  resolveNs: nodeify(resolveNs),
  resolveSrv: nodeify(resolveSrv),
  reverse: nodeify(reverse),
  promises,
  ADDRCONFIG: 0,
  V4MAPPED: 0,
  ALL: 0,
};
