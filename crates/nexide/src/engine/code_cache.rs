//! Persistent V8 bytecode cache for user-bundle compile sites.
//!
//! Stores serialized [`v8::CachedData`] keyed by
//! `SHA-256(source) || cached_data_version_tag()`. Pairs with the
//! aggressive idle-GC pump that flushes V8's internal code: after a
//! `MemoryPressureLevel::Critical` notification V8 may re-parse and
//! re-bytecode-gen on the next request; with the cache hot, that work
//! collapses into a `ConsumeCodeCache` deserialise.
//!
//! ## Storage layout
//!
//! ```text
//! ${NEXIDE_CACHE_DIR:-/tmp/.nexide-cache}/
//!   v1/<v8_tag_hex8>/<sha256_hex64>.bin
//! ```
//!
//! - `v1/` is the on-disk schema version owned by nexide. Bumped when
//!   the file format changes (currently the raw V8 cached-data blob).
//! - `<v8_tag_hex8>/` partitions by V8's bytecode-ABI tag so a
//!   `rusty_v8` upgrade silently invalidates everything from the
//!   previous generation - we never ship a stale blob to a new V8.
//! - File names are content-addressed: identical sources produced by
//!   parallel workers collapse to the same path. Atomic rename
//!   (`<file>.tmp.<pid>` → `<file>`) makes concurrent writes safe.
//!
//! ## Failure model
//!
//! Every cache operation degrades to a no-op on error - reads return
//! `None`, writes log and drop. V8 itself silently falls back to a
//! fresh compile when cached bytes are rejected, so nothing on the
//! hot path can break correctness.
//!
//! ## Concurrency
//!
//! [`CodeCache`] is `Send + Sync` and cheap to clone (`Arc` inside).
//! Stores are dispatched onto Tokio's blocking pool via
//! `tokio::task::spawn_blocking`; the calling V8 thread never blocks
//! on disk. When no Tokio runtime is available (e.g. unit tests),
//! [`Self::store`] falls back to a synchronous write so behaviour
//! stays observable.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};

const LOG_TARGET: &str = "nexide::engine::code_cache";

const SCHEMA_VERSION: &str = "v1";
const FILE_EXT: &str = "bin";

const DEFAULT_QUOTA_MB: u64 = 256;

const ENV_CACHE_DIR: &str = "NEXIDE_CACHE_DIR";
const ENV_KILL_SWITCH: &str = "NEXIDE_CODE_CACHE";
const ENV_QUOTA_MB: &str = "NEXIDE_CACHE_MAX_MB";

/// Per-cache atomic counters. Cheap to read and stable across clones
/// because [`CodeCache`] holds an [`Arc`].
#[derive(Debug, Default)]
pub struct CacheMetrics {
    /// Successful `lookup` calls that produced bytes.
    pub hits: AtomicU64,
    /// `lookup` calls that found nothing on disk.
    pub misses: AtomicU64,
    /// V8 reported `CachedData::rejected()` after a hit - source was
    /// found but bytecode was stale.
    pub rejects: AtomicU64,
    /// Successful `store` writes (including overwrites).
    pub writes: AtomicU64,
    /// `store` calls whose write path returned an I/O error.
    pub write_errors: AtomicU64,
    /// Cumulative bytes written through `store`.
    pub bytes_cached: AtomicU64,
}

impl CacheMetrics {
    fn record_hit(&self) {
        self.hits.fetch_add(1, Ordering::Relaxed);
    }
    fn record_miss(&self) {
        self.misses.fetch_add(1, Ordering::Relaxed);
    }
    /// Logged when V8 marks a freshly loaded cache entry as
    /// `rejected` (typically a tag that survived disk but lost an
    /// internal V8 invariant).
    pub fn record_reject(&self) {
        self.rejects.fetch_add(1, Ordering::Relaxed);
    }
    fn record_write(&self, bytes: u64) {
        self.writes.fetch_add(1, Ordering::Relaxed);
        self.bytes_cached.fetch_add(bytes, Ordering::Relaxed);
    }
    fn record_write_error(&self) {
        self.write_errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Snapshot of all counters. Useful for tracing summaries on
    /// shutdown and for the bench harness.
    #[must_use]
    pub fn snapshot(&self) -> CacheMetricsSnapshot {
        CacheMetricsSnapshot {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            rejects: self.rejects.load(Ordering::Relaxed),
            writes: self.writes.load(Ordering::Relaxed),
            write_errors: self.write_errors.load(Ordering::Relaxed),
            bytes_cached: self.bytes_cached.load(Ordering::Relaxed),
        }
    }
}

/// Plain-old-data snapshot returned by [`CacheMetrics::snapshot`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[allow(missing_docs)]
pub struct CacheMetricsSnapshot {
    pub hits: u64,
    pub misses: u64,
    pub rejects: u64,
    pub writes: u64,
    pub write_errors: u64,
    pub bytes_cached: u64,
}

impl CacheMetricsSnapshot {
    /// Hit ratio in `[0.0, 1.0]`. Returns `0.0` when no lookups have
    /// been recorded so callers can format unconditionally.
    #[must_use]
    pub fn hit_ratio(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }
}

/// Process-wide V8 bytecode cache.
///
/// Cheap to clone: state lives in an [`Arc`]. One instance per
/// runtime is enough; share it across every isolate via the engine
/// handle.
#[derive(Debug, Clone)]
pub struct CodeCache {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    root: PathBuf,
    enabled: bool,
    v8_tag_override: Option<u32>,
    v8_tag: OnceLock<u32>,
    quota_bytes: u64,
    metrics: Arc<CacheMetrics>,
}

impl Inner {
    fn resolve_tag(&self) -> u32 {
        if let Some(tag) = self.v8_tag_override {
            return tag;
        }
        *self
            .v8_tag
            .get_or_init(v8::script_compiler::cached_data_version_tag)
    }
}

impl CodeCache {
    /// Builds a cache from the environment.
    ///
    /// - `NEXIDE_CODE_CACHE=0|false|off|no` → fully disabled.
    /// - `NEXIDE_CACHE_DIR` overrides the storage root (default
    ///   `${TMPDIR}/.nexide-cache`).
    /// - `NEXIDE_CACHE_MAX_MB` caps eviction quota (default 256 MB).
    ///
    /// Best-effort `mkdir -p` is performed up front; failure here
    /// does *not* disable the cache - subsequent reads/writes will
    /// just fail individually and degrade to no-ops.
    #[must_use]
    pub fn from_env() -> Self {
        let kill_switch = std::env::var(ENV_KILL_SWITCH)
            .ok()
            .map(|raw| {
                matches!(
                    raw.trim().to_ascii_lowercase().as_str(),
                    "0" | "false" | "off" | "no"
                )
            })
            .unwrap_or(false);
        if kill_switch {
            return Self::disabled();
        }

        let root = std::env::var(ENV_CACHE_DIR)
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(default_cache_root);

        let quota_bytes = std::env::var(ENV_QUOTA_MB)
            .ok()
            .and_then(|raw| raw.trim().parse::<u64>().ok())
            .filter(|&mb| mb > 0)
            .unwrap_or(DEFAULT_QUOTA_MB)
            .saturating_mul(1024 * 1024);

        let v8_tag_cell: OnceLock<u32> = OnceLock::new();

        if let Err(err) = std::fs::create_dir_all(root.join(SCHEMA_VERSION)) {
            tracing::warn!(
                target: LOG_TARGET,
                path = %root.display(),
                error = %err,
                "code cache: mkdir failed - operations will degrade to no-op"
            );
        }

        Self {
            inner: Arc::new(Inner {
                root,
                enabled: true,
                v8_tag_override: None,
                v8_tag: v8_tag_cell,
                quota_bytes,
                metrics: Arc::new(CacheMetrics::default()),
            }),
        }
    }

    /// Builds a cache pinned to `root` with a custom V8 tag. Test-only
    /// constructor: lets tests assert tag-segregation without
    /// depending on the real V8 ABI tag.
    #[must_use]
    #[doc(hidden)]
    pub fn with_root(root: PathBuf, v8_tag: u32, quota_bytes: u64) -> Self {
        let dir = root.join(SCHEMA_VERSION).join(format!("{v8_tag:08x}"));
        let _ = std::fs::create_dir_all(&dir);
        Self {
            inner: Arc::new(Inner {
                root,
                enabled: true,
                v8_tag_override: Some(v8_tag),
                v8_tag: OnceLock::new(),
                quota_bytes,
                metrics: Arc::new(CacheMetrics::default()),
            }),
        }
    }

    /// Builds a cache pinned to `root` that resolves the V8 ABI tag
    /// lazily on first use, exactly like [`Self::from_env`] but with a
    /// caller-supplied storage root. Test-only.
    #[must_use]
    #[doc(hidden)]
    pub fn with_root_lazy(root: PathBuf, quota_bytes: u64) -> Self {
        let _ = std::fs::create_dir_all(root.join(SCHEMA_VERSION));
        Self {
            inner: Arc::new(Inner {
                root,
                enabled: true,
                v8_tag_override: None,
                v8_tag: OnceLock::new(),
                quota_bytes,
                metrics: Arc::new(CacheMetrics::default()),
            }),
        }
    }

    /// Builds a permanently-disabled cache. All operations are
    /// no-ops; metrics still tick (`misses` only) so dashboards stay
    /// consistent across deployments.
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            inner: Arc::new(Inner {
                root: PathBuf::new(),
                enabled: false,
                v8_tag_override: Some(0),
                v8_tag: OnceLock::new(),
                quota_bytes: 0,
                metrics: Arc::new(CacheMetrics::default()),
            }),
        }
    }

    /// Returns `true` when the cache will read/write the filesystem.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.inner.enabled
    }

    /// Per-cache shared metrics handle.
    #[must_use]
    pub fn metrics(&self) -> Arc<CacheMetrics> {
        Arc::clone(&self.inner.metrics)
    }

    /// V8 bytecode-ABI tag used to partition the cache.
    #[must_use]
    pub fn v8_tag(&self) -> u32 {
        self.inner.resolve_tag()
    }

    /// Storage root - useful for tests.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.inner.root
    }

    /// Returns the absolute path used to store the cache entry for
    /// `source`. Public for tests; not stable.
    #[must_use]
    #[doc(hidden)]
    pub fn entry_path(&self, source: &str) -> PathBuf {
        let mut hasher = Sha256::new();
        hasher.update(source.as_bytes());
        let digest = hasher.finalize();
        let key = hex::encode(digest);
        let tag = self.inner.resolve_tag();
        self.inner
            .root
            .join(SCHEMA_VERSION)
            .join(format!("{tag:08x}"))
            .join(format!("{key}.{FILE_EXT}"))
    }

    /// Reads the cache entry for `source`. Returns `None` on miss or
    /// I/O error.
    pub fn lookup(&self, source: &str) -> Option<Vec<u8>> {
        if !self.inner.enabled {
            self.inner.metrics.record_miss();
            return None;
        }
        let path = self.entry_path(source);
        match std::fs::read(&path) {
            Ok(bytes) if !bytes.is_empty() => {
                self.inner.metrics.record_hit();
                tracing::trace!(
                    target: LOG_TARGET,
                    path = %path.display(),
                    bytes = bytes.len(),
                    "code cache hit"
                );
                Some(bytes)
            }
            Ok(_) => {
                self.inner.metrics.record_miss();
                None
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                self.inner.metrics.record_miss();
                None
            }
            Err(err) => {
                self.inner.metrics.record_miss();
                tracing::debug!(
                    target: LOG_TARGET,
                    path = %path.display(),
                    error = %err,
                    "code cache: lookup failed"
                );
                None
            }
        }
    }

    /// Persists `bytes` for `source`. Asynchronous when a Tokio
    /// runtime is available, synchronous otherwise. Errors are logged
    /// and counted but never propagated - the V8 hot path stays free.
    pub fn store(&self, source: &str, bytes: Vec<u8>) {
        if !self.inner.enabled || bytes.is_empty() {
            return;
        }
        let path = self.entry_path(source);
        let metrics = Arc::clone(&self.inner.metrics);
        let bytes_len = bytes.len() as u64;

        let blocking = move || match write_atomic(&path, &bytes) {
            Ok(()) => {
                metrics.record_write(bytes_len);
                tracing::trace!(
                    target: LOG_TARGET,
                    path = %path.display(),
                    bytes = bytes_len,
                    "code cache store"
                );
            }
            Err(err) => {
                metrics.record_write_error();
                tracing::debug!(
                    target: LOG_TARGET,
                    path = %path.display(),
                    error = %err,
                    "code cache: store failed"
                );
            }
        };

        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn_blocking(blocking);
            }
            Err(_) => blocking(),
        }
    }

    /// Drops oldest entries (by mtime) until the cache fits the
    /// configured quota. Returns the number of files removed.
    /// Designed to be called from the idle-GC pump - never the hot
    /// path. Cheap when below quota.
    pub fn evict_to_quota(&self) -> usize {
        if !self.inner.enabled {
            return 0;
        }
        let tag = self.inner.resolve_tag();
        let dir = self
            .inner
            .root
            .join(SCHEMA_VERSION)
            .join(format!("{tag:08x}"));
        evict_to_quota_in(&dir, self.inner.quota_bytes)
    }
}

fn default_cache_root() -> PathBuf {
    let base = std::env::var("TMPDIR")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join(".nexide-cache")
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let pid = std::process::id();
    let nonce: u64 = {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    };
    let tmp = path.with_extension(format!("tmp.{pid}.{nonce}"));
    std::fs::write(&tmp, bytes)?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = std::fs::remove_file(&tmp);
            Err(err)
        }
    }
}

fn evict_to_quota_in(dir: &Path, quota_bytes: u64) -> usize {
    let entries = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(_) => return 0,
    };
    let mut files: Vec<(PathBuf, u64, std::time::SystemTime)> = Vec::new();
    let mut total: u64 = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let size = meta.len();
        let mtime = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        total = total.saturating_add(size);
        files.push((path, size, mtime));
    }
    if total <= quota_bytes {
        return 0;
    }
    files.sort_by_key(|(_, _, m)| *m);
    let mut removed = 0usize;
    let mut current = total;
    for (path, size, _) in files {
        if current <= quota_bytes {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            removed += 1;
            current = current.saturating_sub(size);
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn disabled_cache_is_noop() {
        let cache = CodeCache::disabled();
        assert!(!cache.is_enabled());
        assert!(cache.lookup("anything").is_none());
        cache.store("anything", vec![1, 2, 3]);
        let snap = cache.metrics().snapshot();
        assert_eq!(snap.hits, 0);
        assert_eq!(snap.writes, 0);
    }

    #[test]
    fn store_then_lookup_roundtrips_identical_bytes() {
        let dir = TempDir::new().unwrap();
        let cache = CodeCache::with_root(dir.path().to_path_buf(), 0xdead_beef, 64 * 1024 * 1024);
        let src = "module.exports = 1";
        let blob = vec![9u8; 128];
        cache.store(src, blob.clone());
        let got = cache.lookup(src).expect("hit");
        assert_eq!(got, blob);
        let snap = cache.metrics().snapshot();
        assert_eq!(snap.hits, 1);
        assert_eq!(snap.writes, 1);
        assert_eq!(snap.bytes_cached, 128);
    }

    #[test]
    fn lookup_miss_returns_none_and_increments_misses() {
        let dir = TempDir::new().unwrap();
        let cache = CodeCache::with_root(dir.path().to_path_buf(), 1, 1 << 20);
        assert!(cache.lookup("nope").is_none());
        let snap = cache.metrics().snapshot();
        assert_eq!(snap.misses, 1);
        assert_eq!(snap.hits, 0);
    }

    #[test]
    fn entry_paths_are_partitioned_by_v8_tag() {
        let dir = TempDir::new().unwrap();
        let a = CodeCache::with_root(dir.path().to_path_buf(), 1, 1 << 20);
        let b = CodeCache::with_root(dir.path().to_path_buf(), 2, 1 << 20);
        a.store("same source", vec![1; 8]);
        assert!(a.lookup("same source").is_some());
        assert!(b.lookup("same source").is_none());
    }

    #[test]
    fn entry_paths_differ_per_source_via_sha256() {
        let dir = TempDir::new().unwrap();
        let cache = CodeCache::with_root(dir.path().to_path_buf(), 7, 1 << 20);
        let p1 = cache.entry_path("foo");
        let p2 = cache.entry_path("bar");
        assert_ne!(p1, p2);
    }

    #[test]
    fn evict_to_quota_drops_oldest_first() {
        let dir = TempDir::new().unwrap();
        let cache = CodeCache::with_root(dir.path().to_path_buf(), 0, 200);
        cache.store("a", vec![1; 90]);
        std::thread::sleep(std::time::Duration::from_millis(20));
        cache.store("b", vec![2; 90]);
        std::thread::sleep(std::time::Duration::from_millis(20));
        cache.store("c", vec![3; 90]);
        let removed = cache.evict_to_quota();
        assert!(removed >= 1, "expected eviction (removed = {removed})");
        assert!(
            cache.lookup("a").is_none() || cache.lookup("b").is_none(),
            "oldest entry must be gone"
        );
        assert!(cache.lookup("c").is_some(), "newest entry must survive");
    }

    #[test]
    fn corrupt_zero_byte_file_is_treated_as_miss() {
        let dir = TempDir::new().unwrap();
        let cache = CodeCache::with_root(dir.path().to_path_buf(), 42, 1 << 20);
        let p = cache.entry_path("empty");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, b"").unwrap();
        assert!(cache.lookup("empty").is_none());
        let snap = cache.metrics().snapshot();
        assert_eq!(snap.hits, 0);
        assert_eq!(snap.misses, 1);
    }

    #[test]
    fn metrics_hit_ratio_zero_when_idle() {
        let snap = CacheMetricsSnapshot::default();
        assert!((snap.hit_ratio() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn metrics_hit_ratio_computes_against_total_lookups() {
        let snap = CacheMetricsSnapshot {
            hits: 3,
            misses: 1,
            ..Default::default()
        };
        assert!((snap.hit_ratio() - 0.75).abs() < f64::EPSILON);
    }

    #[test]
    fn from_env_kill_switch_disables_cache() {
        let restore = EnvGuard::set(ENV_KILL_SWITCH, "0");
        let cache = CodeCache::from_env();
        assert!(!cache.is_enabled());
        drop(restore);
    }

    struct EnvGuard {
        key: &'static str,
        prev: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            // SAFETY: tests in this file run on a single thread per
            // `cargo test` invocation - rust insta-mut env access is
            // sound under that assumption.
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, prev }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: see `EnvGuard::set`.
            unsafe {
                if let Some(ref v) = self.prev {
                    std::env::set_var(self.key, v);
                } else {
                    std::env::remove_var(self.key);
                }
            }
        }
    }
}
