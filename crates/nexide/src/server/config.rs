//! Validated configuration for the `nexide` HTTP shield.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use thiserror::Error;

/// Errors produced while validating a [`ServerConfig`].
#[derive(Debug, Error)]
pub enum ConfigError {
    /// The configured `public_dir` does not exist on disk.
    #[error("public_dir does not exist: {0}")]
    PublicDirMissing(PathBuf),

    /// The configured `next_static_dir` does not exist on disk.
    #[error("next_static_dir does not exist: {0}")]
    NextStaticDirMissing(PathBuf),

    /// The configured `app_dir` does not exist on disk.
    #[error("app_dir does not exist: {0}")]
    AppDirMissing(PathBuf),

    /// `public_dir` must be absolute so that path resolution stays
    /// deterministic across the multi-threaded runtime.
    #[error("public_dir must be absolute: {0}")]
    PublicDirRelative(PathBuf),

    /// `next_static_dir` must be absolute for the same reason as
    /// [`ConfigError::PublicDirRelative`].
    #[error("next_static_dir must be absolute: {0}")]
    NextStaticDirRelative(PathBuf),

    /// `app_dir` must be absolute for the same reason as
    /// [`ConfigError::PublicDirRelative`].
    #[error("app_dir must be absolute: {0}")]
    AppDirRelative(PathBuf),
}

/// Configuration of the Rust shield: bind address plus the directories
/// that are streamed zero-copy via [`tower_http::services::ServeDir`]
/// or short-circuited by the prerender hot path.
///
/// The struct is immutable after construction; mutation goes through a
/// fresh [`ServerConfig::try_new`] call (CQS).
#[derive(Debug, Clone)]
pub struct ServerConfig {
    bind: SocketAddr,
    public_dir: PathBuf,
    next_static_dir: PathBuf,
    app_dir: PathBuf,
}

impl ServerConfig {
    /// Builds a validated configuration.
    ///
    /// `app_dir` is the path to `.next/server/app` produced by
    /// `next build`; it powers the prerender hot path that bypasses
    /// V8 for cacheable HTML/RSC payloads.
    ///
    /// `public_dir` is allowed to be absent on disk - `public/` is an
    /// optional Next.js convention and small/SaaS-style apps often
    /// ship without it. The path is still validated as absolute so
    /// downstream code can join URL fragments deterministically; a
    /// non-existent directory simply yields 404s through `ServeDir`.
    ///
    /// # Errors
    /// [`ConfigError::PublicDirRelative`] /
    /// [`ConfigError::NextStaticDirRelative`] /
    /// [`ConfigError::AppDirRelative`] when a path is not absolute.
    /// [`ConfigError::NextStaticDirMissing`] /
    /// [`ConfigError::AppDirMissing`] when a directory does not exist
    /// at validation time.
    pub fn try_new(
        bind: SocketAddr,
        public_dir: PathBuf,
        next_static_dir: PathBuf,
        app_dir: PathBuf,
    ) -> Result<Self, ConfigError> {
        if !public_dir.is_absolute() {
            return Err(ConfigError::PublicDirRelative(public_dir));
        }
        if !next_static_dir.is_absolute() {
            return Err(ConfigError::NextStaticDirRelative(next_static_dir));
        }
        if !app_dir.is_absolute() {
            return Err(ConfigError::AppDirRelative(app_dir));
        }
        if !next_static_dir.is_dir() {
            return Err(ConfigError::NextStaticDirMissing(next_static_dir));
        }
        if !app_dir.is_dir() {
            return Err(ConfigError::AppDirMissing(app_dir));
        }
        Ok(Self {
            bind,
            public_dir,
            next_static_dir,
            app_dir,
        })
    }

    /// Returns the address the server should bind to. Pure query.
    #[must_use]
    pub const fn bind(&self) -> SocketAddr {
        self.bind
    }

    /// Returns the directory holding `public/` assets. Pure query.
    #[must_use]
    pub fn public_dir(&self) -> &Path {
        self.public_dir.as_path()
    }

    /// Returns the directory holding `_next/static/` chunks. Pure query.
    #[must_use]
    pub fn next_static_dir(&self) -> &Path {
        self.next_static_dir.as_path()
    }

    /// Returns the directory holding prerendered app router assets
    /// (`.next/server/app`). Pure query.
    #[must_use]
    pub fn app_dir(&self) -> &Path {
        self.app_dir.as_path()
    }
}

#[cfg(test)]
mod tests {
    use super::{ConfigError, ServerConfig};
    use std::net::SocketAddr;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn addr() -> SocketAddr {
        "127.0.0.1:0".parse().expect("addr literal")
    }

    fn three_dirs() -> (TempDir, TempDir, TempDir) {
        (
            TempDir::new().expect("tempdir"),
            TempDir::new().expect("tempdir"),
            TempDir::new().expect("tempdir"),
        )
    }

    #[test]
    fn try_new_accepts_existing_absolute_dirs() {
        let (pub_dir, static_dir, app_dir) = three_dirs();
        let cfg = ServerConfig::try_new(
            addr(),
            pub_dir.path().to_path_buf(),
            static_dir.path().to_path_buf(),
            app_dir.path().to_path_buf(),
        )
        .expect("valid config");
        assert_eq!(cfg.public_dir(), pub_dir.path());
        assert_eq!(cfg.next_static_dir(), static_dir.path());
        assert_eq!(cfg.app_dir(), app_dir.path());
    }

    #[test]
    fn try_new_rejects_relative_public_dir() {
        let (_p, static_dir, app_dir) = three_dirs();
        let err = ServerConfig::try_new(
            addr(),
            PathBuf::from("relative/path"),
            static_dir.path().to_path_buf(),
            app_dir.path().to_path_buf(),
        )
        .unwrap_err();
        assert!(matches!(err, ConfigError::PublicDirRelative(_)));
    }

    #[test]
    fn try_new_rejects_relative_next_static_dir() {
        let (pub_dir, _s, app_dir) = three_dirs();
        let err = ServerConfig::try_new(
            addr(),
            pub_dir.path().to_path_buf(),
            PathBuf::from("relative"),
            app_dir.path().to_path_buf(),
        )
        .unwrap_err();
        assert!(matches!(err, ConfigError::NextStaticDirRelative(_)));
    }

    #[test]
    fn try_new_rejects_relative_app_dir() {
        let (pub_dir, static_dir, _a) = three_dirs();
        let err = ServerConfig::try_new(
            addr(),
            pub_dir.path().to_path_buf(),
            static_dir.path().to_path_buf(),
            PathBuf::from("relative/app"),
        )
        .unwrap_err();
        assert!(matches!(err, ConfigError::AppDirRelative(_)));
    }

    #[test]
    fn try_new_accepts_missing_public_dir() {
        let (_p, static_dir, app_dir) = three_dirs();
        let cfg = ServerConfig::try_new(
            addr(),
            PathBuf::from("/definitely/does/not/exist/nexide"),
            static_dir.path().to_path_buf(),
            app_dir.path().to_path_buf(),
        )
        .expect("public_dir is optional");
        assert_eq!(
            cfg.public_dir(),
            std::path::Path::new("/definitely/does/not/exist/nexide"),
        );
    }

    #[test]
    fn try_new_rejects_missing_next_static_dir() {
        let (pub_dir, _s, app_dir) = three_dirs();
        let err = ServerConfig::try_new(
            addr(),
            pub_dir.path().to_path_buf(),
            PathBuf::from("/definitely/does/not/exist/nexide-static"),
            app_dir.path().to_path_buf(),
        )
        .unwrap_err();
        assert!(matches!(err, ConfigError::NextStaticDirMissing(_)));
    }

    #[test]
    fn try_new_rejects_missing_app_dir() {
        let (pub_dir, static_dir, _a) = three_dirs();
        let err = ServerConfig::try_new(
            addr(),
            pub_dir.path().to_path_buf(),
            static_dir.path().to_path_buf(),
            PathBuf::from("/definitely/does/not/exist/nexide-app"),
        )
        .unwrap_err();
        assert!(matches!(err, ConfigError::AppDirMissing(_)));
    }
}
