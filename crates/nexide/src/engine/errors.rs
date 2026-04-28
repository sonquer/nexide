//! Error type produced by the engine layer.
//!
//! All variants carry a stable display prefix (`engine: …`) so that
//! integration tests can assert on textual error contracts without
//! coupling to the underlying V8 error tree.

use std::path::PathBuf;

use thiserror::Error;

/// Failures observable while booting and operating a JavaScript engine.
///
/// The variants intentionally reduce the rich V8 exception
/// hierarchy down to three categories that the rest of the runtime can
/// react to meaningfully:
///
/// * [`EngineError::Bootstrap`] — V8 / `JsRuntime` could not be created.
/// * [`EngineError::ModuleResolution`] — the entrypoint cannot be located
///   or read from disk.
/// * [`EngineError::JsRuntime`] — a JavaScript-level failure (uncaught
///   exception, parse error, evaluation termination, ...).
#[derive(Debug, Error)]
pub enum EngineError {
    /// `JsRuntime` initialization failed before any user code ran.
    #[error("engine: bootstrap failed: {message}")]
    Bootstrap {
        /// Human-readable explanation of the bootstrap failure.
        message: String,
    },

    /// The requested entrypoint could not be resolved on disk.
    #[error("engine: module not resolvable: {path}")]
    ModuleResolution {
        /// Path that the engine attempted to resolve.
        path: PathBuf,
    },

    /// JavaScript-level failure surfaced from the V8 isolate.
    #[error("engine: js runtime error: {message}")]
    JsRuntime {
        /// Stringified V8 exception (kept as `String` so the public
        /// API does not leak `v8::*` types).
        message: String,
    },
}

impl EngineError {
    /// Stable prefix shared by every [`EngineError`] display string.
    ///
    /// Tests assert on this prefix to detect regressions in the public
    /// error contract without pinning to specific wording.
    pub const DISPLAY_PREFIX: &'static str = "engine: ";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_display_uses_stable_prefix() {
        let err = EngineError::Bootstrap {
            message: "v8 init".to_owned(),
        };
        assert!(err.to_string().starts_with(EngineError::DISPLAY_PREFIX));
        assert!(err.to_string().contains("bootstrap failed"));
    }

    #[test]
    fn module_resolution_display_includes_path() {
        let err = EngineError::ModuleResolution {
            path: PathBuf::from("/nope/missing.mjs"),
        };
        assert!(err.to_string().contains("/nope/missing.mjs"));
    }

    #[test]
    fn js_runtime_display_uses_stable_prefix() {
        let err = EngineError::JsRuntime {
            message: "boom".to_owned(),
        };
        assert!(err.to_string().starts_with(EngineError::DISPLAY_PREFIX));
        assert!(err.to_string().contains("boom"));
    }
}
