//! CommonJS specifier resolver.
//!
//! `parent` is the absolute path of the caller, or the sentinel
//! [`ROOT_PARENT`] when `require` is invoked from the top-level
//! entrypoint. The implementation follows a pragmatic subset of the
//! Node.js algorithm: relative paths,
//! `index.*`, `package.json#main`, and `node_modules` walk.

#![allow(clippy::doc_markdown)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::errors::CjsError;
use super::registry::BuiltinRegistry;

/// Sentinel parent value used by the JS loader for top-level
/// `require` calls. The resolver treats it as the project root.
pub const ROOT_PARENT: &str = "<root>";

/// Outcome of [`CjsResolver::resolve`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    /// Concrete file containing CommonJS source.
    File(PathBuf),
    /// Concrete file containing JSON.
    Json(PathBuf),
    /// Native (`.node`) addon loaded via N-API.
    Native(PathBuf),
    /// Built-in module (`node:<name>`).
    Builtin(String),
}

impl Resolved {
    /// Wire format used for op transport: absolute path for files,
    /// `node:<name>` for built-ins.
    #[must_use]
    pub fn to_specifier(&self) -> String {
        match self {
            Self::File(p) | Self::Json(p) | Self::Native(p) => p.to_string_lossy().into_owned(),
            Self::Builtin(name) => format!("node:{name}"),
        }
    }

    /// Inverse of [`Self::to_specifier`].
    ///
    /// # Errors
    ///
    /// [`CjsError::NotFound`] when the wire string is not a valid
    /// specifier.
    pub fn from_specifier(spec: &str) -> Result<Self, CjsError> {
        if let Some(name) = spec.strip_prefix("node:") {
            return Ok(Self::Builtin(name.to_owned()));
        }
        let path = PathBuf::from(spec);
        if !path.is_absolute() {
            return Err(CjsError::NotFound {
                request: spec.to_owned(),
                parent: ROOT_PARENT.to_owned(),
            });
        }
        if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
        {
            Ok(Self::Json(path))
        } else if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("node"))
        {
            Ok(Self::Native(path))
        } else {
            Ok(Self::File(path))
        }
    }
}

/// Resolves `require(...)` specifiers (Query - pure).
pub trait CjsResolver: Send + Sync + 'static {
    /// Resolves `request` from the perspective of `parent`.
    ///
    /// # Errors
    ///
    /// [`CjsError::NotFound`] when no candidate matches,
    /// [`CjsError::AccessDenied`] when the candidate is outside the
    /// configured roots.
    fn resolve(&self, parent: &str, request: &str) -> Result<Resolved, CjsError>;

    /// Returns the source of a `node:*` builtin, or `None` if no
    /// builtin with that name is registered.
    fn builtin_source(&self, name: &str) -> Option<&'static str>;

    /// Reports whether `path` lies inside one of the resolver's
    /// configured roots.
    ///
    /// Used by the op layer to re-validate file specifiers that the JS
    /// guest hands back through `op_cjs_read_source`. The default
    /// implementation accepts everything - only the production
    /// [`FsResolver`] enforces a real boundary; test resolvers usually
    /// run unsandboxed.
    fn is_path_admitted(&self, _path: &Path) -> bool {
        true
    }
}

/// File-system-backed resolver scoped to a list of project roots.
pub struct FsResolver {
    roots: Vec<PathBuf>,
    registry: Arc<BuiltinRegistry>,
}

impl FsResolver {
    /// Builds a resolver pinned to `roots`. The first root is treated
    /// as the project root for [`ROOT_PARENT`] resolutions.
    ///
    /// # Panics
    ///
    /// Panics when `roots` is empty.
    #[must_use]
    pub fn new(roots: Vec<PathBuf>, registry: Arc<BuiltinRegistry>) -> Self {
        assert!(!roots.is_empty(), "FsResolver requires at least one root");
        Self { roots, registry }
    }

    fn project_root(&self) -> &Path {
        &self.roots[0]
    }

    fn parent_dir(&self, parent: &str) -> PathBuf {
        if parent == ROOT_PARENT || parent.starts_with("node:") {
            return self.project_root().to_path_buf();
        }
        let raw = Path::new(parent)
            .parent()
            .map_or_else(|| self.project_root().to_path_buf(), Path::to_path_buf);
        raw.canonicalize().unwrap_or(raw)
    }

    /// Public accessor for the private root-containment check.
    ///
    /// Exposed so callers (notably `op_cjs_read_source`) can re-validate
    /// arbitrary specifiers handed back by JS.
    #[must_use]
    pub fn within_roots_pub(&self, path: &Path) -> bool {
        self.within_roots(path)
    }

    fn within_roots(&self, path: &Path) -> bool {
        let canonical = path.canonicalize().ok();
        let probe = canonical.as_deref().unwrap_or(path);
        self.roots.iter().any(|root| {
            let rc = root.canonicalize().ok();
            let rprobe = rc.as_deref().unwrap_or(root);
            probe.starts_with(rprobe)
        })
    }

    fn classify(path: PathBuf) -> Resolved {
        let path = path.canonicalize().unwrap_or(path);
        if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
        {
            Resolved::Json(path)
        } else if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("node"))
        {
            Resolved::Native(path)
        } else {
            Resolved::File(path)
        }
    }

    fn try_extensions(base: &Path) -> Option<PathBuf> {
        const CANDIDATES: &[&str] = &["js", "cjs", "json", "mjs", "node"];
        if base.is_file() {
            return Some(base.to_path_buf());
        }
        let base_str = base.as_os_str().to_owned();
        CANDIDATES.iter().find_map(|ext| {
            let mut appended = base_str.clone();
            appended.push(".");
            appended.push(ext);
            let candidate = PathBuf::from(appended);
            if candidate.is_file() {
                return Some(candidate);
            }
            let replaced = base.with_extension(ext);
            replaced.is_file().then_some(replaced)
        })
    }

    fn try_directory(dir: &Path) -> Option<PathBuf> {
        if !dir.is_dir() {
            return None;
        }
        let pkg = dir.join("package.json");
        if pkg.is_file()
            && let Ok(text) = std::fs::read_to_string(&pkg)
            && let Ok(json) = serde_json::from_str::<serde_json::Value>(&text)
            && let Some(main) = json.get("main").and_then(serde_json::Value::as_str)
        {
            let main_path = dir.join(main);
            if let Some(found) = Self::try_extensions(&main_path) {
                return Some(found);
            }
            let with_index = main_path.join("index");
            if let Some(found) = Self::try_extensions(&with_index) {
                return Some(found);
            }
        }
        let index = dir.join("index");
        Self::try_extensions(&index)
    }

    fn resolve_file_path(&self, base_dir: &Path, request: &str) -> Result<Resolved, CjsError> {
        let raw = if Path::new(request).is_absolute() {
            PathBuf::from(request)
        } else {
            base_dir.join(request)
        };
        let candidate = Self::try_extensions(&raw).or_else(|| Self::try_directory(&raw));
        let path = candidate.ok_or_else(|| CjsError::NotFound {
            request: request.to_owned(),
            parent: base_dir.to_string_lossy().into_owned(),
        })?;
        if !self.within_roots(&path) {
            return Err(CjsError::AccessDenied(path));
        }
        Ok(Self::classify(path))
    }

    fn resolve_node_modules(&self, base_dir: &Path, request: &str) -> Result<Resolved, CjsError> {
        let (pkg_name, subpath) = split_package_name(request);
        let mut current = Some(base_dir.to_path_buf());
        while let Some(dir) = current {
            let pkg_dir = dir.join("node_modules").join(&pkg_name);
            if pkg_dir.is_dir()
                && let Some(found) = Self::resolve_in_package(&pkg_dir, subpath)
                && self.within_roots(&found)
            {
                return Ok(Self::classify(found));
            }
            current = dir.parent().map(Path::to_path_buf);
        }
        Err(CjsError::NotFound {
            request: request.to_owned(),
            parent: base_dir.to_string_lossy().into_owned(),
        })
    }

    fn resolve_in_package(pkg_dir: &Path, subpath: &str) -> Option<PathBuf> {
        let pkg_path = pkg_dir.join("package.json");
        let pkg_json = if pkg_path.is_file() {
            std::fs::read_to_string(&pkg_path)
                .ok()
                .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        } else {
            None
        };

        if let Some(json) = &pkg_json
            && let Some(exports) = json.get("exports")
        {
            let key = if subpath.is_empty() {
                ".".to_owned()
            } else {
                format!("./{subpath}")
            };
            if let Some(rel) = match_exports(exports, &key) {
                let candidate = pkg_dir.join(rel.trim_start_matches("./"));
                if candidate.is_file() {
                    return Some(candidate);
                }
                if let Some(found) = Self::try_extensions(&candidate) {
                    return Some(found);
                }
            }
        }

        if subpath.is_empty() {
            if let Some(json) = &pkg_json
                && let Some(main) = json.get("main").and_then(serde_json::Value::as_str)
            {
                let main_path = pkg_dir.join(main);
                if let Some(found) = Self::try_extensions(&main_path) {
                    return Some(found);
                }
                let with_index = main_path.join("index");
                if let Some(found) = Self::try_extensions(&with_index) {
                    return Some(found);
                }
            }
            let index = pkg_dir.join("index");
            return Self::try_extensions(&index);
        }

        let candidate = pkg_dir.join(subpath);
        Self::try_extensions(&candidate).or_else(|| Self::try_directory(&candidate))
    }
}

fn split_package_name(request: &str) -> (String, &str) {
    request.strip_prefix('@').map_or_else(
        || {
            let mut parts = request.splitn(2, '/');
            let name = parts.next().unwrap_or("").to_owned();
            let rest = parts.next().unwrap_or("");
            (name, rest)
        },
        |stripped| {
            let mut parts = stripped.splitn(3, '/');
            let scope = parts.next().unwrap_or("");
            let name = parts.next().unwrap_or("");
            let rest = parts.next().unwrap_or("");
            (format!("@{scope}/{name}"), rest)
        },
    )
}

fn match_exports(exports: &serde_json::Value, key: &str) -> Option<String> {
    if let Some(obj) = exports.as_object() {
        if let Some(direct) = obj.get(key)
            && let Some(s) = pick_condition(direct)
        {
            return Some(s);
        }
        for (pat, val) in obj {
            if let Some(star_idx) = pat.find('*') {
                let (prefix, suffix) = pat.split_at(star_idx);
                let suffix = &suffix[1..];
                if key.starts_with(prefix) && key.ends_with(suffix) && key.len() >= pat.len() - 1 {
                    let middle = &key[prefix.len()..key.len() - suffix.len()];
                    if let Some(template) = pick_condition(val) {
                        return Some(template.replacen('*', middle, 1));
                    }
                }
            }
        }
        return None;
    }
    if let Some(s) = exports.as_str()
        && key == "."
    {
        return Some(s.to_owned());
    }
    None
}

fn pick_condition(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Object(map) => {
            for cond in ["require", "node", "default"] {
                if let Some(v) = map.get(cond)
                    && let Some(s) = pick_condition(v)
                {
                    return Some(s);
                }
            }
            None
        }
        _ => None,
    }
}

impl CjsResolver for FsResolver {
    fn resolve(&self, parent: &str, request: &str) -> Result<Resolved, CjsError> {
        if let Some(name) = request.strip_prefix("node:") {
            if self.registry.contains(name) {
                return Ok(Resolved::Builtin(name.to_owned()));
            }
            return Err(CjsError::NotFound {
                request: request.to_owned(),
                parent: parent.to_owned(),
            });
        }

        let is_relative = request.starts_with("./")
            || request.starts_with("../")
            || request.starts_with('/')
            || request == "."
            || request == "..";

        if !is_relative && self.registry.contains(request) {
            return Ok(Resolved::Builtin(request.to_owned()));
        }

        let base_dir = self.parent_dir(parent);
        if is_relative {
            self.resolve_file_path(&base_dir, request)
        } else {
            self.resolve_node_modules(&base_dir, request)
        }
    }

    fn builtin_source(&self, name: &str) -> Option<&'static str> {
        self.registry.lookup(name).map(|m| m.source())
    }

    fn is_path_admitted(&self, path: &Path) -> bool {
        self.within_roots_pub(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use crate::engine::cjs::registry::BuiltinModule;

    struct Stub {
        name: &'static str,
    }
    impl BuiltinModule for Stub {
        fn name(&self) -> &'static str {
            self.name
        }
        fn source(&self) -> &'static str {
            "module.exports = {};"
        }
    }

    fn tmp_root() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    fn registry_with(names: &[&'static str]) -> Arc<BuiltinRegistry> {
        let mut reg = BuiltinRegistry::new();
        for n in names {
            reg.register(Arc::new(Stub { name: n })).expect("register");
        }
        Arc::new(reg)
    }

    #[test]
    fn resolves_node_prefix_to_builtin() {
        let dir = tmp_root();
        let resolver = FsResolver::new(vec![dir.path().to_path_buf()], registry_with(&["path"]));
        let r = resolver.resolve(ROOT_PARENT, "node:path").expect("ok");
        assert_eq!(r, Resolved::Builtin("path".to_owned()));
    }

    #[test]
    fn resolves_bare_to_builtin_when_registered() {
        let dir = tmp_root();
        let resolver = FsResolver::new(vec![dir.path().to_path_buf()], registry_with(&["path"]));
        let r = resolver.resolve(ROOT_PARENT, "path").expect("ok");
        assert_eq!(r, Resolved::Builtin("path".to_owned()));
    }

    #[test]
    fn unknown_node_prefix_errors() {
        let dir = tmp_root();
        let resolver = FsResolver::new(vec![dir.path().to_path_buf()], registry_with(&[]));
        let err = resolver.resolve(ROOT_PARENT, "node:nope").unwrap_err();
        assert!(matches!(err, CjsError::NotFound { .. }));
    }

    #[test]
    fn resolves_relative_with_extension_probe() {
        let dir = tmp_root();
        fs::write(dir.path().join("foo.js"), "module.exports = 1;").expect("write");
        let resolver = FsResolver::new(vec![dir.path().to_path_buf()], registry_with(&[]));
        let r = resolver.resolve(ROOT_PARENT, "./foo").expect("ok");
        assert!(matches!(r, Resolved::File(p) if p.ends_with("foo.js")));
    }

    #[test]
    fn resolves_relative_json() {
        let dir = tmp_root();
        fs::write(dir.path().join("data.json"), "{}").expect("write");
        let resolver = FsResolver::new(vec![dir.path().to_path_buf()], registry_with(&[]));
        let r = resolver.resolve(ROOT_PARENT, "./data.json").expect("ok");
        assert!(matches!(r, Resolved::Json(_)));
    }

    #[test]
    fn resolves_directory_index() {
        let dir = tmp_root();
        let sub = dir.path().join("pkg");
        fs::create_dir(&sub).expect("mkdir");
        fs::write(sub.join("index.js"), "module.exports = 'pkg';").expect("write");
        let resolver = FsResolver::new(vec![dir.path().to_path_buf()], registry_with(&[]));
        let r = resolver.resolve(ROOT_PARENT, "./pkg").expect("ok");
        assert!(matches!(r, Resolved::File(p) if p.ends_with("index.js")));
    }

    #[test]
    fn resolves_directory_main_via_package_json() {
        let dir = tmp_root();
        let sub = dir.path().join("pkg");
        fs::create_dir(&sub).expect("mkdir");
        fs::write(sub.join("package.json"), r#"{"main": "lib/entry.js"}"#).expect("pkg");
        fs::create_dir(sub.join("lib")).expect("mkdir lib");
        fs::write(sub.join("lib").join("entry.js"), "module.exports = 'main';").expect("entry");
        let resolver = FsResolver::new(vec![dir.path().to_path_buf()], registry_with(&[]));
        let r = resolver.resolve(ROOT_PARENT, "./pkg").expect("ok");
        assert!(matches!(r, Resolved::File(p) if p.ends_with("entry.js")));
    }

    #[test]
    fn relative_resolution_fails_for_missing() {
        let dir = tmp_root();
        let resolver = FsResolver::new(vec![dir.path().to_path_buf()], registry_with(&[]));
        let err = resolver.resolve(ROOT_PARENT, "./missing").unwrap_err();
        assert!(matches!(err, CjsError::NotFound { .. }));
    }

    #[test]
    fn rejects_paths_outside_roots() {
        let dir = tmp_root();
        let outside = tmp_root();
        fs::write(outside.path().join("evil.js"), "throw new Error('boom');").expect("write");
        let resolver = FsResolver::new(vec![dir.path().to_path_buf()], registry_with(&[]));
        let outside_path = outside.path().join("evil.js");
        let outside_str = outside_path.to_string_lossy().into_owned();
        let err = resolver.resolve(ROOT_PARENT, &outside_str).unwrap_err();
        assert!(matches!(err, CjsError::AccessDenied(_)));
    }

    #[test]
    fn node_modules_walk_finds_package() {
        let dir = tmp_root();
        let nm = dir.path().join("node_modules").join("lib");
        fs::create_dir_all(&nm).expect("mkdir");
        fs::write(nm.join("index.js"), "module.exports = 7;").expect("write");
        let resolver = FsResolver::new(vec![dir.path().to_path_buf()], registry_with(&[]));
        let r = resolver.resolve(ROOT_PARENT, "lib").expect("ok");
        assert!(matches!(r, Resolved::File(p) if p.ends_with("index.js")));
    }

    #[test]
    fn specifier_round_trips() {
        let r = Resolved::File(PathBuf::from("/abs/foo.js"));
        assert_eq!(Resolved::from_specifier(&r.to_specifier()).unwrap(), r);

        let j = Resolved::Json(PathBuf::from("/abs/data.json"));
        assert_eq!(Resolved::from_specifier(&j.to_specifier()).unwrap(), j);

        let b = Resolved::Builtin("path".to_owned());
        assert_eq!(Resolved::from_specifier(&b.to_specifier()).unwrap(), b);
    }
}
