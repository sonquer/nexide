"use strict";

// node:os - backed by `nexide_os_ops`. The op layer holds the
// `OsInfoSource` so production and test isolates differ only at the
// Rust seam.

const ops = Nexide.core.ops;

const EOL = (typeof globalThis.process !== "undefined"
  && globalThis.process.platform === "win32") ? "\r\n" : "\n";

module.exports = {
  EOL,
  arch() { return ops.op_os_arch(); },
  platform() { return ops.op_os_platform(); },
  type() { return ops.op_os_type(); },
  release() { return ops.op_os_release(); },
  version() { return ops.op_os_release(); },
  hostname() { return ops.op_os_hostname(); },
  tmpdir() { return ops.op_os_tmpdir(); },
  homedir() { return ops.op_os_homedir() || ""; },
  endianness() { return ops.op_os_endianness(); },
  uptime() { return Number(ops.op_os_uptime_secs()); },
  freemem() { return Number(ops.op_os_freemem()); },
  totalmem() { return Number(ops.op_os_totalmem()); },
  cpus() {
    const n = ops.op_os_cpus_count();
    const out = [];
    for (let i = 0; i < n; i++) {
      out.push({
        model: "unknown",
        speed: 0,
        times: { user: 0, nice: 0, sys: 0, idle: 0, irq: 0 },
      });
    }
    return out;
  },
  networkInterfaces() { return {}; },
  loadavg() { return [0, 0, 0]; },
  userInfo() {
    return {
      username: "nexide",
      uid: -1,
      gid: -1,
      shell: null,
      homedir: ops.op_os_homedir() || "",
    };
  },
  constants: {
    signals: {},
    errno: {},
    priority: {},
    dlopen: {
      RTLD_LAZY: 0x1,
      RTLD_NOW: 0x2,
      RTLD_GLOBAL: 0x8,
      RTLD_LOCAL: 0x4,
      RTLD_DEEPBIND: 0x10,
    },
  },
};
