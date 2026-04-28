//! Built-in `node:*` module catalogue.
//!
//! Encapsulates the static JS sources shipped with nexide and exposes
//! a single [`register_node_builtins`] helper used by the engine bootstrap.
//! Tests can build a registry and inspect / require any module without
//! repeating the wiring.

use std::sync::Arc;

use super::errors::CjsError;
use super::registry::{BuiltinModule, BuiltinRegistry};

/// Static built-in module backed by an embedded `&'static str` source.
struct StaticModule {
    name: &'static str,
    source: &'static str,
}

impl BuiltinModule for StaticModule {
    fn name(&self) -> &'static str {
        self.name
    }
    fn source(&self) -> &'static str {
        self.source
    }
}

/// Compile-time table of `(module name, source)` pairs shipped by nexide.
///
/// The names match the canonical Node.js module identifier without the
/// `node:` prefix; the resolver accepts both forms automatically.
const MODULES: &[(&str, &str)] = &[
    (
        "assert",
        include_str!("../../../runtime/polyfills/node/assert.js"),
    ),
    (
        "async_hooks",
        include_str!("../../../runtime/polyfills/node/async_hooks.js"),
    ),
    (
        "path",
        include_str!("../../../runtime/polyfills/node/path.js"),
    ),
    (
        "url",
        include_str!("../../../runtime/polyfills/node/url.js"),
    ),
    (
        "querystring",
        include_str!("../../../runtime/polyfills/node/querystring.js"),
    ),
    (
        "util",
        include_str!("../../../runtime/polyfills/node/util.js"),
    ),
    ("os", include_str!("../../../runtime/polyfills/node/os.js")),
    (
        "process",
        include_str!("../../../runtime/polyfills/node/process.js"),
    ),
    (
        "events",
        include_str!("../../../runtime/polyfills/node/events.js"),
    ),
    (
        "buffer",
        include_str!("../../../runtime/polyfills/node/buffer.js"),
    ),
    (
        "stream",
        include_str!("../../../runtime/polyfills/node/stream.js"),
    ),
    ("fs", include_str!("../../../runtime/polyfills/node/fs.js")),
    (
        "fs/promises",
        include_str!("../../../runtime/polyfills/node/fs-promises.js"),
    ),
    (
        "zlib",
        include_str!("../../../runtime/polyfills/node/zlib.js"),
    ),
    (
        "crypto",
        include_str!("../../../runtime/polyfills/node/crypto.js"),
    ),
    (
        "http",
        include_str!("../../../runtime/polyfills/node/http.js"),
    ),
    (
        "https",
        include_str!("../../../runtime/polyfills/node/https.js"),
    ),
    (
        "inspector",
        include_str!("../../../runtime/polyfills/node/inspector.js"),
    ),
    (
        "net",
        include_str!("../../../runtime/polyfills/node/net.js"),
    ),
    (
        "tls",
        include_str!("../../../runtime/polyfills/node/tls.js"),
    ),
    (
        "timers",
        include_str!("../../../runtime/polyfills/node/timers.js"),
    ),
    (
        "timers/promises",
        include_str!("../../../runtime/polyfills/node/timers_promises.js"),
    ),
    (
        "dns",
        include_str!("../../../runtime/polyfills/node/dns.js"),
    ),
    (
        "dns/promises",
        include_str!("../../../runtime/polyfills/node/dns_promises.js"),
    ),
    (
        "child_process",
        include_str!("../../../runtime/polyfills/node/child_process.js"),
    ),
    (
        "constants",
        include_str!("../../../runtime/polyfills/node/constants.js"),
    ),
    (
        "worker_threads",
        include_str!("../../../runtime/polyfills/node/worker_threads.js"),
    ),
    ("vm", include_str!("../../../runtime/polyfills/node/vm.js")),
    ("v8", include_str!("../../../runtime/polyfills/node/v8.js")),
    (
        "string_decoder",
        include_str!("../../../runtime/polyfills/node/string_decoder.js"),
    ),
    (
        "module",
        include_str!("../../../runtime/polyfills/node/module.js"),
    ),
    (
        "perf_hooks",
        include_str!("../../../runtime/polyfills/node/perf_hooks.js"),
    ),
    (
        "tty",
        include_str!("../../../runtime/polyfills/node/tty.js"),
    ),
];

/// Registers every shipped `node:*` module on `registry`.
///
/// # Errors
///
/// [`CjsError::DuplicateBuiltin`] if a module with the same name is
/// already present - surfaces wiring bugs (e.g. registering the
/// catalogue twice).
pub fn register_node_builtins(registry: &mut BuiltinRegistry) -> Result<(), CjsError> {
    for (name, source) in MODULES {
        registry.register(Arc::new(StaticModule { name, source }))?;
    }
    Ok(())
}

/// Convenience constructor returning a registry preloaded with every
/// shipped `node:*` module.
///
/// # Errors
///
/// Propagates [`register_node_builtins`].
pub fn default_registry() -> Result<BuiltinRegistry, CjsError> {
    let mut reg = BuiltinRegistry::new();
    register_node_builtins(&mut reg)?;
    Ok(reg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_registry_lists_every_shipped_module() {
        let reg = default_registry().expect("ok");
        let mut names = reg.names();
        names.sort();
        assert_eq!(
            names,
            vec![
                "assert",
                "async_hooks",
                "buffer",
                "child_process",
                "constants",
                "crypto",
                "dns",
                "dns/promises",
                "events",
                "fs",
                "fs/promises",
                "http",
                "https",
                "inspector",
                "module",
                "net",
                "os",
                "path",
                "perf_hooks",
                "process",
                "querystring",
                "stream",
                "string_decoder",
                "timers",
                "timers/promises",
                "tls",
                "tty",
                "url",
                "util",
                "v8",
                "vm",
                "worker_threads",
                "zlib",
            ],
        );
    }

    #[test]
    fn register_twice_is_rejected() {
        let mut reg = default_registry().expect("first");
        let err = register_node_builtins(&mut reg);
        assert!(matches!(err, Err(CjsError::DuplicateBuiltin(_))));
    }

    #[test]
    fn every_module_source_is_non_empty() {
        let reg = default_registry().expect("ok");
        for name in reg.names() {
            let m = reg.lookup(&name).expect("registered");
            assert!(!m.source().is_empty(), "module {name} has empty source");
        }
    }
}
