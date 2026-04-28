//! Entrypoint resolution for the production runtime.
//!
//! The runtime is driven by the Next.js standalone bundle produced by
//! `next build` with `output: 'standalone'`. The resolver locates
//! `<root>/.next/standalone/server.js` on disk; missing layouts are
//! reported as `None` so the caller can surface a precise error.
//!
//! Selection is decoupled from filesystem access via the
//! [`EntrypointResolver`] trait so tests can inject deterministic
//! choices without touching the disk.

use std::path::{Path, PathBuf};

/// Outcome of [`EntrypointResolver::resolve`] - both the absolute path
/// to the JS entrypoint and a tag identifying which branch was taken
/// (used for tracing and the `next_version` boot log).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedEntrypoint {
    /// Absolute path to the JS file the isolate should load.
    pub path: PathBuf,
    /// Which production layout fed the path.
    pub kind: EntrypointKind,
}

/// Branches the [`EntrypointResolver`] can pick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntrypointKind {
    /// `<root>/.next/standalone/server.js` (real Next.js).
    NextStandalone,
}

impl EntrypointKind {
    /// Stable, machine-friendly label used in tracing.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::NextStandalone => "next_standalone",
        }
    }
}

/// Resolves the JS entrypoint the runtime should boot the isolate
/// pool against.
///
/// Implementors are pure command-query: a single
/// [`resolve`](Self::resolve) call must always return the same answer
/// for the same observable filesystem state.
pub trait EntrypointResolver: Send + Sync {
    /// Returns the entrypoint to load, or `None` when the standalone
    /// bundle is missing.
    fn resolve(&self) -> Option<ResolvedEntrypoint>;
}

/// Filesystem-backed resolver: probes `<root>/.next/standalone/server.js`.
///
/// The path is pre-computed at construction time; the resolve step is
/// a single `Path::is_file` check and never mutates state.
#[derive(Debug, Clone)]
pub struct ProductionEntrypointResolver {
    standalone_path: PathBuf,
}

impl ProductionEntrypointResolver {
    /// Builds a resolver rooted at the example-app directory `root`.
    #[must_use]
    pub fn new(root: &Path) -> Self {
        Self {
            standalone_path: root.join(".next/standalone/server.js"),
        }
    }

    /// Returns the absolute path the resolver will probe.
    #[must_use]
    pub fn standalone_path(&self) -> &Path {
        &self.standalone_path
    }
}

impl EntrypointResolver for ProductionEntrypointResolver {
    fn resolve(&self) -> Option<ResolvedEntrypoint> {
        if self.standalone_path.is_file() {
            return Some(ResolvedEntrypoint {
                path: self.standalone_path.clone(),
                kind: EntrypointKind::NextStandalone,
            });
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{
        EntrypointKind, EntrypointResolver, ProductionEntrypointResolver, ResolvedEntrypoint,
    };
    use std::path::Path;

    fn write(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(path, body).expect("write");
    }

    #[test]
    fn resolves_standalone_when_present() {
        let dir = tempfile::tempdir().expect("tmp");
        write(
            &dir.path().join(".next/standalone/server.js"),
            "// next standalone",
        );

        let resolved: ResolvedEntrypoint = ProductionEntrypointResolver::new(dir.path())
            .resolve()
            .expect("entrypoint");
        assert_eq!(resolved.kind, EntrypointKind::NextStandalone);
        assert!(resolved.path.ends_with(".next/standalone/server.js"));
    }

    #[test]
    fn returns_none_when_standalone_missing() {
        let dir = tempfile::tempdir().expect("tmp");
        let resolver = ProductionEntrypointResolver::new(dir.path());
        assert!(resolver.resolve().is_none());
    }

    #[test]
    fn label_is_stable() {
        assert_eq!(EntrypointKind::NextStandalone.label(), "next_standalone");
    }
}
