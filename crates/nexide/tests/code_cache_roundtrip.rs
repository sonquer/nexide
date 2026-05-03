//! End-to-end test for the V8 bytecode code-cache.
//!
//! Boots a real isolate against a `CodeCache` rooted at a `TempDir`,
//! runs a CJS entry that `require()`s a sibling module, and asserts:
//!
//! 1. First boot is a miss + write (cache file appears on disk).
//! 2. Second boot with the same source is a hit (no extra writes).
//! 3. Mutating the required module forces a fresh miss.
//! 4. Corrupting an existing cache file produces a reject + rewrite.

#![allow(clippy::future_not_send, clippy::significant_drop_tightening)]

use std::path::Path;
use std::sync::Arc;

use nexide::engine::cjs::{FsResolver, default_registry};
use nexide::engine::{BootContext, CodeCache, V8Engine};

async fn boot(dir: &Path, entry: &Path, cache: CodeCache) -> Result<(), String> {
    let registry = Arc::new(default_registry().expect("default registry"));
    let resolver = Arc::new(FsResolver::new(vec![dir.to_path_buf()], registry));
    let ctx = BootContext::new().with_cjs(resolver).with_code_cache(cache);
    V8Engine::boot_with(entry, ctx)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

async fn run_with_cache(dir: &Path, entry: &Path, cache: CodeCache) -> CodeCache {
    let local = tokio::task::LocalSet::new();
    let dir_buf = dir.to_path_buf();
    let entry_buf = entry.to_path_buf();
    let cache_clone = cache.clone();
    let result = local
        .run_until(async move { boot(&dir_buf, &entry_buf, cache_clone).await })
        .await;
    result.unwrap_or_else(|e| panic!("boot failed: {e}"));
    cache
}

fn make_cache(root: &Path) -> CodeCache {
    CodeCache::with_root_lazy(root.to_path_buf(), 64 * 1024 * 1024)
}

fn count_cache_files(root: &Path) -> usize {
    fn walk(p: &Path, acc: &mut usize) {
        let Ok(rd) = std::fs::read_dir(p) else { return };
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, acc);
            } else if path.extension().is_some_and(|e| e == "bin") {
                *acc += 1;
            }
        }
    }
    let mut acc = 0;
    walk(root, &mut acc);
    acc
}

fn collect_cache_files(root: &Path) -> Vec<std::path::PathBuf> {
    fn walk(p: &Path, acc: &mut Vec<std::path::PathBuf>) {
        let Ok(rd) = std::fs::read_dir(p) else { return };
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, acc);
            } else if path.extension().is_some_and(|e| e == "bin") {
                acc.push(path);
            }
        }
    }
    let mut acc = Vec::new();
    walk(root, &mut acc);
    acc
}

#[tokio::test(flavor = "current_thread")]
async fn cache_roundtrip_hit_after_first_boot() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cache_dir = tempfile::tempdir().expect("cache tempdir");

    std::fs::write(
        dir.path().join("dep.cjs"),
        "module.exports = { add: (a, b) => a + b };",
    )
    .expect("write dep");
    let entry = dir.path().join("entry.cjs");
    std::fs::write(
        &entry,
        r#"
            const dep = require('./dep.cjs');
            if (dep.add(2, 3) !== 5) {
                throw new Error('arith broken');
            }
        "#,
    )
    .expect("write entry");

    let cache = make_cache(cache_dir.path());
    let cache = run_with_cache(dir.path(), &entry, cache).await;
    let snap1 = cache.metrics().snapshot();
    assert_eq!(snap1.hits, 0, "first boot must not see cache hits");
    assert!(
        snap1.writes >= 2,
        "first boot must persist entries: {snap1:?}"
    );
    assert!(
        count_cache_files(cache_dir.path()) >= 2,
        "expected cache files on disk after first boot"
    );

    let cache2 = make_cache(cache_dir.path());
    let cache2 = run_with_cache(dir.path(), &entry, cache2).await;
    let snap2 = cache2.metrics().snapshot();
    assert!(
        snap2.hits >= 2,
        "second boot must hit cache for entry+dep, got {snap2:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn mutating_source_forces_miss() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cache_dir = tempfile::tempdir().expect("cache tempdir");

    let entry = dir.path().join("entry.cjs");
    std::fs::write(&entry, "module.exports = 1;").expect("write entry v1");

    let cache = make_cache(cache_dir.path());
    let _ = run_with_cache(dir.path(), &entry, cache).await;
    let files_v1 = count_cache_files(cache_dir.path());
    assert!(files_v1 >= 1, "v1 should write at least one entry");

    std::fs::write(&entry, "module.exports = 2; // mutated").expect("write entry v2");
    let cache2 = make_cache(cache_dir.path());
    let cache2 = run_with_cache(dir.path(), &entry, cache2).await;
    let snap = cache2.metrics().snapshot();
    assert!(snap.misses >= 1, "mutated source must miss, snap={snap:?}");
    let files_v2 = count_cache_files(cache_dir.path());
    assert!(
        files_v2 > files_v1,
        "mutated source must add a new cache file (v1={files_v1}, v2={files_v2})"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn corrupted_cache_file_rejected_and_rewritten() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cache_dir = tempfile::tempdir().expect("cache tempdir");

    let entry = dir.path().join("entry.cjs");
    std::fs::write(&entry, "module.exports = 'hello';").expect("write entry");

    let cache = make_cache(cache_dir.path());
    let _ = run_with_cache(dir.path(), &entry, cache).await;

    let files = collect_cache_files(cache_dir.path());
    assert!(!files.is_empty(), "expected at least one cache file");
    for f in &files {
        std::fs::write(f, b"\x00\x01\x02garbage-not-v8-bytecode\xff\xff\xff").expect("corrupt");
    }

    let cache2 = make_cache(cache_dir.path());
    let cache2 = run_with_cache(dir.path(), &entry, cache2).await;
    let snap = cache2.metrics().snapshot();
    assert!(
        snap.rejects >= 1,
        "corrupted cache must produce rejects, snap={snap:?}"
    );
    assert!(
        snap.writes >= 1,
        "rejected entry must be rewritten, snap={snap:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn disabled_cache_records_no_io() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cache_dir = tempfile::tempdir().expect("cache tempdir");

    let entry = dir.path().join("entry.cjs");
    std::fs::write(&entry, "module.exports = 42;").expect("write entry");

    let cache = CodeCache::disabled();
    let cache = run_with_cache(dir.path(), &entry, cache).await;
    let snap = cache.metrics().snapshot();
    assert_eq!(snap.hits, 0);
    assert_eq!(snap.misses, 0);
    assert_eq!(snap.writes, 0);
    assert_eq!(count_cache_files(cache_dir.path()), 0);
}
