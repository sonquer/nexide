"use strict";

// node:querystring — escape/unescape + parse/stringify with array
// repetition support.

function escape(str) { return encodeURIComponent(str); }
function unescape(str) {
  try { return decodeURIComponent(str.replace(/\+/g, " ")); }
  catch { return str; }
}

function stringify(obj, sep, eq) {
  if (obj === null || typeof obj !== "object") return "";
  sep = sep || "&";
  eq = eq || "=";
  const out = [];
  for (const key of Object.keys(obj)) {
    const k = escape(String(key));
    const value = obj[key];
    if (Array.isArray(value)) {
      for (const v of value) out.push(k + eq + escape(String(v)));
    } else if (value !== undefined && value !== null) {
      out.push(k + eq + escape(String(value)));
    } else {
      out.push(k + eq);
    }
  }
  return out.join(sep);
}

function parse(qs, sep, eq) {
  sep = sep || "&";
  eq = eq || "=";
  const obj = Object.create(null);
  if (typeof qs !== "string" || qs.length === 0) return obj;
  for (const pair of qs.split(sep)) {
    const idx = pair.indexOf(eq);
    let k, v;
    if (idx === -1) { k = unescape(pair); v = ""; }
    else { k = unescape(pair.slice(0, idx)); v = unescape(pair.slice(idx + eq.length)); }
    if (Object.hasOwn(obj, k)) {
      if (Array.isArray(obj[k])) obj[k].push(v);
      else obj[k] = [obj[k], v];
    } else {
      obj[k] = v;
    }
  }
  return obj;
}

module.exports = {
  parse,
  stringify,
  escape,
  unescape,
  encode: stringify,
  decode: parse,
};
