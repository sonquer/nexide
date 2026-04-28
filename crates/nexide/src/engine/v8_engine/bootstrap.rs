//! Polyfill bootstrap concatenation.
//!
//! All `crates/nexide/runtime/polyfills/*.js` files are embedded at
//! compile time and concatenated in install order. They are evaluated
//! against the freshly created context **before** the user entrypoint
//! so they observe a clean global scope.

/// Order matters: each polyfill builds on the previous ones.
///
/// 1. `nexide_bridge.js` — wraps raw ops in the `__nexide` façade.
/// 2. `console.js`       — replaces V8 default `console`.
/// 3. `process.js`       — installs `globalThis.process`.
/// 4. `buffer.js`        — Node-style `Buffer`.
/// 5. `timers.js`        — `setTimeout` / `setInterval` /
///    `queueMicrotask`.
/// 6. `web_apis.js`      — `TextEncoder`, `URL`, etc.
/// 7. `async_local_storage.js` — `AsyncLocalStorage`.
/// 8. `late_globals.js`  — final `globalThis.*` shims that
///    intentionally run after every other polyfill.
/// 9. `http_bridge.js`   — receives requests and dispatches to the
///    user-supplied handler installed via
///    `globalThis.__nexide_handler`.
/// 10. `cjs_loader.js`   — `require()` shim used by Next.js bundles.
pub(super) const POLYFILL_SCRIPTS: &[(&str, &str)] = &[
    (
        "nexide:nexide_bridge.js",
        include_str!("../../../runtime/polyfills/nexide_bridge.js"),
    ),
    (
        "nexide:v8_core_shim.js",
        include_str!("../../../runtime/polyfills/v8_core_shim.js"),
    ),
    (
        "nexide:console.js",
        include_str!("../../../runtime/polyfills/console.js"),
    ),
    (
        "nexide:process.js",
        include_str!("../../../runtime/polyfills/process.js"),
    ),
    (
        "nexide:buffer.js",
        include_str!("../../../runtime/polyfills/buffer.js"),
    ),
    (
        "nexide:timers.js",
        include_str!("../../../runtime/polyfills/timers.js"),
    ),
    (
        "nexide:web_apis.js",
        include_str!("../../../runtime/polyfills/web_apis.js"),
    ),
    (
        "nexide:async_local_storage.js",
        include_str!("../../../runtime/polyfills/async_local_storage.js"),
    ),
    (
        "nexide:http_bridge.js",
        include_str!("../../../runtime/polyfills/http_bridge.js"),
    ),
    (
        "nexide:cjs_loader.js",
        include_str!("../../../runtime/polyfills/cjs_loader.js"),
    ),
    (
        "nexide:late_globals.js",
        include_str!("../../../runtime/polyfills/late_globals.js"),
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polyfill_scripts_have_unique_specifiers() {
        let mut seen = std::collections::HashSet::new();
        for (spec, _) in POLYFILL_SCRIPTS {
            assert!(seen.insert(*spec), "duplicate polyfill specifier: {spec}");
        }
    }

    #[test]
    fn polyfill_scripts_are_non_empty() {
        for (spec, src) in POLYFILL_SCRIPTS {
            assert!(!src.is_empty(), "polyfill {spec} is empty");
        }
    }
}
