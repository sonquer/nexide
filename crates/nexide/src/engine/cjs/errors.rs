//! Error variants raised by the CommonJS substrate.

#![allow(clippy::doc_markdown)]

use std::path::PathBuf;

use thiserror::Error;

/// Failure modes returned by the CommonJS resolver, registry, and
/// runtime façade.
#[derive(Debug, Error)]
pub enum CjsError {
    /// Specifier could not be resolved relative to `parent`.
    #[error("MODULE_NOT_FOUND: cannot find '{request}' from '{parent}'")]
    NotFound {
        /// Specifier passed to `require(...)`.
        request: String,
        /// Module that issued the call.
        parent: String,
    },

    /// Resolved path points at a directory without an `index.*`
    /// fallback or a usable `package.json#main`.
    #[error("EISDIR: '{0}' is a directory without entry point")]
    IsDirectory(PathBuf),

    /// Resolved path is outside every configured project root.
    #[error("EACCES: read denied for '{0}'")]
    AccessDenied(PathBuf),

    /// Underlying I/O error while reading module source.
    #[error("EIO: failed to read '{path}': {source}")]
    Io {
        /// Path that could not be read.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// Resolved path points at a native module (`.node`) which the
    /// runtime cannot load.
    #[error("ERR_DLOPEN_FAILED: native modules are not supported in nexide ('{0}')")]
    NativeModule(PathBuf),

    /// Tried to register a built-in twice under the same name.
    #[error("EBUILTIN: built-in '{0}' already registered")]
    DuplicateBuiltin(String),
}
