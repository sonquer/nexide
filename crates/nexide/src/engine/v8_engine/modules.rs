//! ESM module map and resolver wired into V8's `instantiate_module`.
//!
//! Maintains a `HashMap<PathBuf, v8::Global<v8::Module>>` keyed by
//! the absolute filesystem path of each loaded module. Resolution is
//! filesystem-relative - the entrypoint's parent directory is the
//! anchor for relative imports. `node_modules` walks and Next.js
//! bundle rewrites are handled by the resolver layer.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use crate::engine::EngineError;

/// Per-isolate cache of compiled ESM modules.
///
/// Lookup is path-keyed; the same source loaded under two different
/// path normalisations would compile twice. `canonicalize` is *not*
/// applied - symlink-aware semantics belong to the resolver layer
/// (the resolver layer is responsible for symlink-aware semantics).
#[derive(Default)]
pub(super) struct ModuleMap {
    by_path: HashMap<PathBuf, v8::Global<v8::Module>>,
    by_hash: HashMap<i32, PathBuf>,
    resolution: HashMap<(i32, String), PathBuf>,
    synthetic_exports: HashMap<i32, v8::Global<v8::Value>>,
}

impl ModuleMap {
    /// Constructs an empty map.
    #[must_use]
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// Inserts a freshly compiled module. The next [`Self::get`] for
    /// `path` returns the same global handle.
    pub(super) fn insert(
        &mut self,
        path: PathBuf,
        module_hash: i32,
        module: v8::Global<v8::Module>,
    ) {
        self.by_hash.insert(module_hash, path.clone());
        self.by_path.insert(path, module);
    }

    /// Returns the cached module for `path`, if any.
    #[must_use]
    pub(super) fn get(&self, path: &Path) -> Option<&v8::Global<v8::Module>> {
        self.by_path.get(path)
    }

    /// Resolves the absolute path that produced the module with the
    /// supplied identity hash. V8's `Module::get_identity_hash` is the
    /// only key the resolve callback receives because it cannot store
    /// arbitrary embedder data on a module handle.
    #[must_use]
    pub(super) fn path_of_hash(&self, hash: i32) -> Option<&Path> {
        self.by_hash.get(&hash).map(PathBuf::as_path)
    }

    /// Records the resolution `(referrer, specifier) -> abs_key` so
    /// that V8's resolve callback can map module requests back to
    /// modules pre-compiled by the ESM loader.
    pub(super) fn set_resolution(&mut self, referrer_hash: i32, specifier: &str, key: PathBuf) {
        self.resolution
            .insert((referrer_hash, specifier.to_owned()), key);
    }

    /// Looks up a previously recorded resolution.
    #[must_use]
    pub(super) fn lookup_resolution(&self, referrer_hash: i32, specifier: &str) -> Option<&Path> {
        self.resolution
            .get(&(referrer_hash, specifier.to_owned()))
            .map(PathBuf::as_path)
    }

    /// Stashes the CJS exports object for later consumption by the
    /// synthetic-module evaluation steps callback.
    pub(super) fn stash_synthetic_exports(
        &mut self,
        module_hash: i32,
        exports: v8::Global<v8::Value>,
    ) {
        self.synthetic_exports.insert(module_hash, exports);
    }

    /// Pops the stashed exports object for `module_hash`. The callback
    /// only needs them once - V8 invokes evaluation steps a single
    /// time per synthetic module.
    #[must_use]
    pub(super) fn take_synthetic_exports(
        &mut self,
        module_hash: i32,
    ) -> Option<v8::Global<v8::Value>> {
        self.synthetic_exports.remove(&module_hash)
    }
}

/// Joins `parent.parent()` with the relative `specifier` and
/// normalises `..` / `.` segments without touching the filesystem.
///
/// V8 hands resolve callbacks free-form specifier strings (`./foo`,
/// `../bar/baz.mjs`, bare `node:fs`, etc.). This function only handles
/// the relative subset; bare specifiers are passed through verbatim
/// and rejected by the caller as "module not found".
#[must_use]
pub(super) fn resolve_relative(parent: &Path, specifier: &str) -> PathBuf {
    let base = parent.parent().unwrap_or(parent);
    let mut combined = base.to_path_buf();
    combined.push(specifier);
    normalize(&combined)
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// Reads `path` from disk, returning an [`EngineError::ModuleResolution`]
/// when the file cannot be read.
pub(super) fn read_module_source(path: &Path) -> Result<String, EngineError> {
    std::fs::read_to_string(path).map_err(|_| EngineError::ModuleResolution {
        path: path.to_path_buf(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_relative_walks_parent() {
        let parent = Path::new("/srv/app/server.mjs");
        assert_eq!(
            resolve_relative(parent, "./util.mjs"),
            PathBuf::from("/srv/app/util.mjs")
        );
        assert_eq!(
            resolve_relative(parent, "../shared/log.mjs"),
            PathBuf::from("/srv/shared/log.mjs")
        );
    }

    #[test]
    fn module_map_starts_empty() {
        let map = ModuleMap::new();
        assert!(map.path_of_hash(42).is_none());
        assert!(map.get(Path::new("/x")).is_none());
    }

    #[test]
    fn resolve_relative_collapses_dots() {
        let parent = Path::new("/a/b/c.mjs");
        assert_eq!(
            resolve_relative(parent, "./d/./e.mjs"),
            PathBuf::from("/a/b/d/e.mjs")
        );
    }

    #[test]
    fn read_module_source_reports_missing() {
        let err = read_module_source(Path::new("/nope/missing.mjs"));
        assert!(matches!(err, Err(EngineError::ModuleResolution { .. })));
    }
}
