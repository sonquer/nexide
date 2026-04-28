"use strict";

// node:path — POSIX + Win32 implementations sharing helpers.
// Only the surface required by Next.js standalone is covered; behaviour
// matches Node.js for valid inputs.

function assertString(name, value) {
  if (typeof value !== "string") {
    throw new TypeError(
      `Path argument "${name}" must be a string. Received ${typeof value}`,
    );
  }
}

function normalizeStringPosix(path, allowAboveRoot) {
  let res = "";
  let lastSegmentLength = 0;
  let lastSlash = -1;
  let dots = 0;
  let code;
  for (let i = 0; i <= path.length; ++i) {
    if (i < path.length) code = path.charCodeAt(i);
    else if (code === 47) break;
    else code = 47;
    if (code === 47) {
      if (lastSlash !== i - 1 && dots !== 1) {
        if (dots === 2) {
          if (
            res.length < 2 ||
            lastSegmentLength !== 2 ||
            res.charCodeAt(res.length - 1) !== 46 ||
            res.charCodeAt(res.length - 2) !== 46
          ) {
            if (res.length > 2) {
              const lastSlashIndex = res.lastIndexOf("/");
              if (lastSlashIndex !== res.length - 1) {
                res = lastSlashIndex === -1 ? "" : res.slice(0, lastSlashIndex);
                lastSegmentLength = res.length - 1 - res.lastIndexOf("/");
                lastSlash = i;
                dots = 0;
                continue;
              }
            } else if (res.length === 2 || res.length === 1) {
              res = "";
              lastSegmentLength = 0;
              lastSlash = i;
              dots = 0;
              continue;
            }
          }
          if (allowAboveRoot) {
            if (res.length > 0) res += "/..";
            else res = "..";
            lastSegmentLength = 2;
          }
        } else {
          if (res.length > 0) res += "/" + path.slice(lastSlash + 1, i);
          else res = path.slice(lastSlash + 1, i);
          lastSegmentLength = i - lastSlash - 1;
        }
      }
      lastSlash = i;
      dots = 0;
    } else if (code === 46 /*.*/ && dots !== -1) {
      ++dots;
    } else {
      dots = -1;
    }
  }
  return res;
}

const posix = {
  sep: "/",
  delimiter: ":",

  isAbsolute(p) {
    assertString("path", p);
    return p.length > 0 && p.charCodeAt(0) === 47;
  },

  normalize(p) {
    assertString("path", p);
    if (p.length === 0) return ".";
    const isAbs = p.charCodeAt(0) === 47;
    const trailingSep = p.charCodeAt(p.length - 1) === 47;
    let path = normalizeStringPosix(p, !isAbs);
    if (path.length === 0 && !isAbs) path = ".";
    if (path.length > 0 && trailingSep) path += "/";
    return isAbs ? "/" + path : path;
  },

  join(...parts) {
    if (parts.length === 0) return ".";
    let joined;
    for (const arg of parts) {
      assertString("path", arg);
      if (arg.length > 0) {
        if (joined === undefined) joined = arg;
        else joined += "/" + arg;
      }
    }
    if (joined === undefined) return ".";
    return posix.normalize(joined);
  },

  resolve(...parts) {
    let resolved = "";
    let resolvedAbs = false;
    for (let i = parts.length - 1; i >= -1 && !resolvedAbs; i--) {
      const path = i >= 0 ? parts[i] : (globalThis.process && process.cwd()) || "/";
      assertString("path", path);
      if (path.length === 0) continue;
      resolved = path + "/" + resolved;
      resolvedAbs = path.charCodeAt(0) === 47;
    }
    resolved = normalizeStringPosix(resolved, !resolvedAbs);
    if (resolvedAbs) return "/" + resolved;
    return resolved.length > 0 ? resolved : ".";
  },

  relative(from, to) {
    assertString("from", from);
    assertString("to", to);
    if (from === to) return "";
    from = posix.resolve(from);
    to = posix.resolve(to);
    if (from === to) return "";
    const fromParts = from.split("/").filter(Boolean);
    const toParts = to.split("/").filter(Boolean);
    let i = 0;
    while (i < fromParts.length && i < toParts.length && fromParts[i] === toParts[i]) i++;
    const up = fromParts.slice(i).map(() => "..");
    return up.concat(toParts.slice(i)).join("/");
  },

  dirname(p) {
    assertString("path", p);
    if (p.length === 0) return ".";
    let end = -1;
    let matched = false;
    for (let i = p.length - 1; i >= 1; i--) {
      if (p.charCodeAt(i) === 47) {
        if (matched) { end = i; break; }
      } else { matched = true; }
    }
    if (end === -1) return p.charCodeAt(0) === 47 ? "/" : ".";
    if (end === 0) return "/";
    return p.slice(0, end);
  },

  basename(p, ext) {
    assertString("path", p);
    let start = 0;
    let end = p.length;
    for (let i = p.length - 1; i >= 0; i--) {
      if (p.charCodeAt(i) === 47) { start = i + 1; break; }
    }
    let name = p.slice(start, end);
    if (typeof ext === "string" && name.endsWith(ext) && name !== ext) {
      name = name.slice(0, name.length - ext.length);
    }
    return name;
  },

  extname(p) {
    assertString("path", p);
    let startDot = -1;
    let startPart = 0;
    let end = -1;
    let preDotState = 0;
    for (let i = p.length - 1; i >= 0; --i) {
      const code = p.charCodeAt(i);
      if (code === 47) { if (preDotState === 0) { startPart = i + 1; break; } continue; }
      if (end === -1) end = i + 1;
      if (code === 46) {
        if (startDot === -1) startDot = i;
        else if (preDotState !== 1) preDotState = 1;
      } else if (startDot !== -1) preDotState = -1;
    }
    if (startDot === -1 || end === -1 || preDotState === 0 || (preDotState === 1 && startDot === end - 1 && startDot === startPart + 1)) {
      return "";
    }
    return p.slice(startDot, end);
  },

  parse(p) {
    assertString("path", p);
    const root = posix.isAbsolute(p) ? "/" : "";
    const dir = posix.dirname(p);
    const base = posix.basename(p);
    const ext = posix.extname(p);
    const name = base.slice(0, base.length - ext.length);
    return { root, dir: dir === "." ? root : dir, base, ext, name };
  },

  format(parsed) {
    if (parsed === null || typeof parsed !== "object") {
      throw new TypeError('"pathObject" must be an object');
    }
    const dir = parsed.dir || parsed.root || "";
    const base = parsed.base || ((parsed.name || "") + (parsed.ext || ""));
    if (!dir) return base;
    if (dir === parsed.root) return dir + base;
    return dir + "/" + base;
  },
};

const win32 = {
  sep: "\\",
  delimiter: ";",
  isAbsolute(p) {
    assertString("path", p);
    if (p.length === 0) return false;
    const c0 = p.charCodeAt(0);
    if (c0 === 47 || c0 === 92) return true;
    if (p.length >= 3 && p.charCodeAt(1) === 58) {
      const c2 = p.charCodeAt(2);
      if (c2 === 47 || c2 === 92) return true;
    }
    return false;
  },
  normalize(p) {
    assertString("path", p);
    return p.replace(/\//g, "\\");
  },
  join(...parts) {
    return parts.filter((p) => { assertString("path", p); return p.length > 0; }).join("\\");
  },
  resolve(...parts) {
    return win32.join(...parts);
  },
  relative(from, to) { return posix.relative(from, to); },
  dirname(p) { return posix.dirname(p.replace(/\\/g, "/")).replace(/\//g, "\\"); },
  basename(p, ext) { return posix.basename(p.replace(/\\/g, "/"), ext); },
  extname(p) { return posix.extname(p.replace(/\\/g, "/")); },
  parse(p) { return posix.parse(p.replace(/\\/g, "/")); },
  format(parsed) { return posix.format(parsed).replace(/\//g, "\\"); },
};

const isWindows = typeof globalThis.process !== "undefined"
  && globalThis.process.platform === "win32";
const active = isWindows ? win32 : posix;

module.exports = {
  ...active,
  posix,
  win32,
};
