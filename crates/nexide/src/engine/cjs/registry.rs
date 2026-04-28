//! Registry of built-in `node:*` modules exposed to the CommonJS
//! loader.
//!
//! The registry lives behind an [`std::sync::Arc`] and is shared by
//! every isolate. Modules are immutable JavaScript source strings
//! that the loader wraps in the standard CJS function preamble.

#![allow(clippy::doc_markdown)]

use std::collections::HashMap;
use std::sync::Arc;

use super::errors::CjsError;

/// Contract every built-in module must satisfy.
pub trait BuiltinModule: Send + Sync + 'static {
    /// Stable identifier (without the `node:` prefix).
    fn name(&self) -> &'static str;
    /// CommonJS source executed inside the standard wrapper.
    fn source(&self) -> &'static str;
}

/// Thread-safe collection of built-in modules keyed by their name.
#[derive(Default)]
pub struct BuiltinRegistry {
    modules: HashMap<String, Arc<dyn BuiltinModule>>,
}

impl BuiltinRegistry {
    /// Builds an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            modules: HashMap::new(),
        }
    }

    /// Registers `module` under its declared name.
    ///
    /// # Errors
    ///
    /// [`CjsError::DuplicateBuiltin`] if a module with the same name
    /// is already registered.
    pub fn register(&mut self, module: Arc<dyn BuiltinModule>) -> Result<(), CjsError> {
        let name = module.name().to_owned();
        if self.modules.contains_key(&name) {
            return Err(CjsError::DuplicateBuiltin(name));
        }
        self.modules.insert(name, module);
        Ok(())
    }

    /// Returns the module registered under `name`, if any.
    #[must_use]
    pub fn lookup(&self, name: &str) -> Option<&Arc<dyn BuiltinModule>> {
        self.modules.get(name)
    }

    /// Returns `true` when `name` is a registered built-in.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.modules.contains_key(name)
    }

    /// Snapshot of all registered names; allocates on every call.
    #[must_use]
    pub fn names(&self) -> Vec<String> {
        self.modules.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Stub {
        name: &'static str,
        source: &'static str,
    }
    impl BuiltinModule for Stub {
        fn name(&self) -> &'static str {
            self.name
        }
        fn source(&self) -> &'static str {
            self.source
        }
    }

    #[test]
    fn register_then_lookup_returns_module() {
        let mut reg = BuiltinRegistry::new();
        reg.register(Arc::new(Stub {
            name: "path",
            source: "module.exports = {};",
        }))
        .expect("register");
        assert!(reg.contains("path"));
        assert_eq!(reg.lookup("path").expect("found").name(), "path");
    }

    #[test]
    fn register_rejects_duplicates() {
        let mut reg = BuiltinRegistry::new();
        reg.register(Arc::new(Stub {
            name: "path",
            source: "x",
        }))
        .expect("first");
        let err = reg.register(Arc::new(Stub {
            name: "path",
            source: "y",
        }));
        assert!(matches!(err, Err(CjsError::DuplicateBuiltin(_))));
    }

    #[test]
    fn lookup_misses_for_unknown_name() {
        let reg = BuiltinRegistry::new();
        assert!(reg.lookup("no-such").is_none());
        assert!(!reg.contains("no-such"));
    }

    #[test]
    fn names_lists_every_registration() {
        let mut reg = BuiltinRegistry::new();
        reg.register(Arc::new(Stub {
            name: "a",
            source: "",
        }))
        .expect("a");
        reg.register(Arc::new(Stub {
            name: "b",
            source: "",
        }))
        .expect("b");
        let mut names = reg.names();
        names.sort();
        assert_eq!(names, vec!["a", "b"]);
    }
}
