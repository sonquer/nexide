//! Node `process` substrate exposed to the isolate.
//!
//! This module owns three concerns:
//!
//! 1. The [`EnvSource`] abstraction — the only entry point used by the
//!    `op_process_env_*` ops to look up environment variables. Production
//!    binds it to [`OsEnv`] (reads from the parent OS); tests bind it to
//!    [`MapEnv`] (in-memory).
//! 2. [`ProcessConfig`] — a Single Responsibility object that decides
//!    *which* env keys a guest module is allowed to observe. Next.js
//!    code expects `NEXT_*`, `NODE_*`, and `NEXT_PUBLIC_*` to flow
//!    through; everything else stays opaque to the JS world unless an
//!    explicit allow-listed key is configured.
//! 3. The op definitions themselves plus the [`nexide_process_ops`]
//!    extension — pure transport, all logic lives in the layers above.
//!
//! All ops are pure Queries except [`op_process_exit`], the lone
//! Command, which writes a single [`ExitRequested`] marker into
//! `OpState`. The runtime never terminates the host process directly;
//! the marker exists so the embedder can decide what to do.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Instant;

/// Per-isolate writable overlay over [`ProcessConfig`].
///
/// The JS guest can mutate `process.env` (e.g. Next.js' standalone
/// `server.js` sets `__NEXT_PRIVATE_STANDALONE_CONFIG`). Those writes
/// must be observable on subsequent reads, but they should not leak
/// across isolates and must never touch the host OS environment.
///
/// Stored entries:
///   * `Some(value)` — the JS guest set this key.
///   * `None`        — the JS guest deleted this key (shadows the
///     [`ProcessConfig`] value, if any).
#[derive(Debug, Default)]
#[allow(clippy::option_option, clippy::redundant_pub_crate)]
pub struct EnvOverlay {
    entries: RefCell<HashMap<String, Option<String>>>,
}

impl EnvOverlay {
    /// Records that `key` was set to `value` by the guest.
    pub fn set(&self, key: String, value: String) {
        self.entries.borrow_mut().insert(key, Some(value));
    }

    /// Records that `key` was deleted by the guest.
    pub fn delete(&self, key: String) {
        self.entries.borrow_mut().insert(key, None);
    }

    /// Returns `Some(Some(v))` for an overlay-set value, `Some(None)`
    /// for an overlay-deleted key, or `None` when the overlay has no
    /// opinion about `key`.
    #[must_use]
    #[allow(clippy::option_option)]
    pub fn lookup(&self, key: &str) -> Option<Option<String>> {
        self.entries.borrow().get(key).cloned()
    }

    /// Materialises a `(key -> value)` snapshot of currently-set
    /// overlay entries (deleted keys are excluded).
    #[must_use]
    pub fn live_entries(&self) -> Vec<(String, String)> {
        self.entries
            .borrow()
            .iter()
            .filter_map(|(k, v)| v.as_ref().map(|val| (k.clone(), val.clone())))
            .collect()
    }

    /// Returns every key explicitly deleted by the guest.
    #[must_use]
    pub fn deleted_keys(&self) -> Vec<String> {
        self.entries
            .borrow()
            .iter()
            .filter(|(_, v)| v.is_none())
            .map(|(k, _)| k.clone())
            .collect()
    }
}

/// Stable identifiers used internally by [`ProcessConfig`] when
/// constructing the visibility whitelist. Kept as constants so the
/// list is easy to audit and extend.
const DEFAULT_PREFIXES: &[&str] = &["NEXT_", "NODE_", "NEXT_PUBLIC_"];
const DEFAULT_KEYS: &[&str] = &["TZ", "LANG", "LC_ALL", "PATH", "HOME", "PWD"];

/// Read-only view over an environment variable backend.
///
/// Implementations must be cheap to clone (typically `Arc<...>`) and
/// thread-safe. The trait is the seam between the runtime config layer
/// and the op layer — production-only `std::env` access is intentionally
/// confined to [`OsEnv`].
pub trait EnvSource: Send + Sync + 'static {
    /// Returns the value bound to `key`, or `None` when unset.
    fn get(&self, key: &str) -> Option<String>;

    /// Returns every key currently bound. Order is unspecified.
    fn keys(&self) -> Vec<String>;
}

/// [`EnvSource`] backed by the host operating system via [`std::env`].
#[derive(Debug, Default, Clone, Copy)]
pub struct OsEnv;

impl EnvSource for OsEnv {
    fn get(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }

    fn keys(&self) -> Vec<String> {
        std::env::vars().map(|(k, _)| k).collect()
    }
}

/// In-memory [`EnvSource`] used by tests and embeddings that want to
/// inject a controlled environment.
#[derive(Debug, Default, Clone)]
pub struct MapEnv {
    inner: HashMap<String, String>,
}

impl MapEnv {
    /// Builds a [`MapEnv`] from any iterator of `(key, value)` pairs.
    pub fn from_pairs<I, K, V>(pairs: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        Self {
            inner: pairs
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        }
    }
}

impl EnvSource for MapEnv {
    fn get(&self, key: &str) -> Option<String> {
        self.inner.get(key).cloned()
    }

    fn keys(&self) -> Vec<String> {
        self.inner.keys().cloned().collect()
    }
}

/// Visibility policy for the JS-side `process.env` proxy.
///
/// The struct is built once per runtime (typically in `serve_until`)
/// and shared with every isolate via [`OpState`]. It owns the
/// [`EnvSource`] behind an [`Arc`] so it can be cloned cheaply.
#[derive(Clone)]
pub struct ProcessConfig {
    env: Arc<dyn EnvSource>,
    prefixes: Vec<String>,
    keys: BTreeSet<String>,
    cwd: String,
    boot: Instant,
    platform: &'static str,
    arch: &'static str,
}

impl std::fmt::Debug for ProcessConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessConfig")
            .field("prefixes", &self.prefixes)
            .field("keys", &self.keys)
            .field("cwd", &self.cwd)
            .field("platform", &self.platform)
            .field("arch", &self.arch)
            .finish()
    }
}

/// Builder façade for [`ProcessConfig`]. Keeps the type immutable once
/// constructed so the op layer never has to take a write lock.
pub struct ProcessConfigBuilder {
    env: Arc<dyn EnvSource>,
    prefixes: Vec<String>,
    keys: BTreeSet<String>,
    cwd: Option<String>,
}

impl ProcessConfigBuilder {
    /// Creates a builder with the production defaults: `NEXT_*`,
    /// `NODE_*`, `NEXT_PUBLIC_*` prefixes plus a small set of
    /// universally-safe keys (`TZ`, `LANG`, …).
    #[must_use]
    pub fn new(env: Arc<dyn EnvSource>) -> Self {
        Self {
            env,
            prefixes: DEFAULT_PREFIXES.iter().map(|p| (*p).to_owned()).collect(),
            keys: DEFAULT_KEYS.iter().map(|k| (*k).to_owned()).collect(),
            cwd: None,
        }
    }

    /// Adds `prefix` to the visibility whitelist.
    #[must_use]
    pub fn allow_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefixes.push(prefix.into());
        self
    }

    /// Adds `key` to the explicit allow-list.
    #[must_use]
    pub fn allow_key(mut self, key: impl Into<String>) -> Self {
        self.keys.insert(key.into());
        self
    }

    /// Overrides the value reported as `process.cwd()`. Defaults to
    /// the host process working directory.
    #[must_use]
    pub fn with_cwd(mut self, cwd: impl Into<String>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Materialises the configuration.
    #[must_use]
    pub fn build(self) -> ProcessConfig {
        let cwd = self.cwd.unwrap_or_else(|| {
            std::env::current_dir()
                .ok()
                .and_then(|p| p.into_os_string().into_string().ok())
                .unwrap_or_default()
        });
        ProcessConfig {
            env: self.env,
            prefixes: self.prefixes,
            keys: self.keys,
            cwd,
            boot: Instant::now(),
            platform: host_platform(),
            arch: host_arch(),
        }
    }
}

impl ProcessConfig {
    /// Returns a builder seeded with production defaults.
    #[must_use]
    pub fn builder(env: Arc<dyn EnvSource>) -> ProcessConfigBuilder {
        ProcessConfigBuilder::new(env)
    }

    /// Convenience constructor: production defaults bound to [`OsEnv`].
    #[must_use]
    pub fn from_os() -> Self {
        Self::builder(Arc::new(OsEnv)).build()
    }

    /// Returns whether `key` is visible to the JS world.
    #[must_use]
    pub fn is_visible(&self, key: &str) -> bool {
        if self.keys.contains(key) {
            return true;
        }
        self.prefixes.iter().any(|p| key.starts_with(p))
    }

    /// Looks up `key` and returns its value when visible.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<String> {
        if !self.is_visible(key) {
            return None;
        }
        self.env.get(key)
    }

    /// Lists every visible key currently bound.
    #[must_use]
    pub fn visible_keys(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .env
            .keys()
            .into_iter()
            .filter(|k| self.is_visible(k))
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// Reports `process.cwd()`.
    #[must_use]
    pub fn cwd(&self) -> &str {
        &self.cwd
    }

    /// Reports `process.platform` (Node-compatible string).
    #[must_use]
    pub const fn platform(&self) -> &'static str {
        self.platform
    }

    /// Reports `process.arch` (Node-compatible string).
    #[must_use]
    pub const fn arch(&self) -> &'static str {
        self.arch
    }

    /// Returns nanoseconds elapsed since this config was constructed.
    /// Used as the monotonic clock backing `process.hrtime.bigint`.
    #[must_use]
    pub fn hrtime_ns(&self) -> u64 {
        u64::try_from(self.boot.elapsed().as_nanos()).unwrap_or(u64::MAX)
    }
}

/// Marker placed in [`OpState`] when the JS guest calls
/// `process.exit(code)`. The embedder is expected to harvest it after
/// the event loop drains; nexide itself never aborts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitRequested(pub i32);

/// Returns the Node-compatible platform string for the host.
const fn host_platform() -> &'static str {
    if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "windows") {
        "win32"
    } else if cfg!(target_os = "freebsd") {
        "freebsd"
    } else {
        "unknown"
    }
}

/// Returns the Node-compatible architecture string for the host.
const fn host_arch() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "x64"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else if cfg!(target_arch = "x86") {
        "ia32"
    } else if cfg!(target_arch = "arm") {
        "arm"
    } else {
        "unknown"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with(pairs: &[(&str, &str)]) -> ProcessConfig {
        let env = Arc::new(MapEnv::from_pairs(
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned())),
        ));
        ProcessConfig::builder(env).build()
    }

    #[test]
    fn map_env_returns_none_for_missing_key() {
        let env = MapEnv::from_pairs([("FOO", "bar")]);
        assert_eq!(env.get("FOO"), Some("bar".to_owned()));
        assert_eq!(env.get("MISSING"), None);
    }

    #[test]
    fn whitelist_default_prefixes_let_next_keys_through() {
        let cfg = cfg_with(&[
            ("NEXT_PUBLIC_FOO", "1"),
            ("NODE_ENV", "production"),
            ("SECRET_TOKEN", "shh"),
        ]);
        assert_eq!(cfg.get("NEXT_PUBLIC_FOO"), Some("1".to_owned()));
        assert_eq!(cfg.get("NODE_ENV"), Some("production".to_owned()));
        assert_eq!(cfg.get("SECRET_TOKEN"), None);
    }

    #[test]
    fn whitelist_explicit_keys_are_visible() {
        let cfg = cfg_with(&[("PATH", "/usr/bin"), ("RANDOM_THING", "x")]);
        assert_eq!(cfg.get("PATH"), Some("/usr/bin".to_owned()));
        assert_eq!(cfg.get("RANDOM_THING"), None);
    }

    #[test]
    fn whitelist_custom_prefix_extends_visibility() {
        let env = Arc::new(MapEnv::from_pairs([("MY_FOO", "ok"), ("OTHER", "no")]));
        let cfg = ProcessConfig::builder(env).allow_prefix("MY_").build();
        assert_eq!(cfg.get("MY_FOO"), Some("ok".to_owned()));
        assert_eq!(cfg.get("OTHER"), None);
    }

    #[test]
    fn whitelist_custom_key_overrides_default_filter() {
        let env = Arc::new(MapEnv::from_pairs([("CUSTOM", "yes")]));
        let cfg = ProcessConfig::builder(env).allow_key("CUSTOM").build();
        assert_eq!(cfg.get("CUSTOM"), Some("yes".to_owned()));
    }

    #[test]
    fn visible_keys_filters_and_sorts() {
        let cfg = cfg_with(&[
            ("NEXT_PUBLIC_B", "1"),
            ("NEXT_PUBLIC_A", "1"),
            ("HIDDEN", "1"),
        ]);
        let visible = cfg.visible_keys();
        assert!(visible.contains(&"NEXT_PUBLIC_A".to_owned()));
        assert!(visible.contains(&"NEXT_PUBLIC_B".to_owned()));
        assert!(!visible.contains(&"HIDDEN".to_owned()));
        let mut sorted = visible.clone();
        sorted.sort();
        assert_eq!(visible, sorted);
    }

    #[test]
    fn hrtime_ns_is_monotonic() {
        let cfg = cfg_with(&[]);
        let a = cfg.hrtime_ns();
        std::thread::sleep(std::time::Duration::from_millis(1));
        let b = cfg.hrtime_ns();
        assert!(b > a, "expected monotonic clock, got {a} -> {b}");
    }

    #[test]
    fn cwd_override_takes_precedence_over_host() {
        let env = Arc::new(MapEnv::default());
        let cfg = ProcessConfig::builder(env).with_cwd("/tmp/nexide").build();
        assert_eq!(cfg.cwd(), "/tmp/nexide");
    }

    #[test]
    fn platform_and_arch_match_host_targets() {
        let cfg = cfg_with(&[]);
        assert!(matches!(
            cfg.platform(),
            "linux" | "darwin" | "win32" | "freebsd" | "unknown"
        ));
        assert!(matches!(
            cfg.arch(),
            "x64" | "arm64" | "ia32" | "arm" | "unknown"
        ));
    }
}
