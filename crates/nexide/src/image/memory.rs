//! Tiny in-process LRU for optimized image bytes.
//!
//! Sits in front of the on-disk cache so repeated requests for the
//! same `(href, w, q, mime)` triple don't pay the syscall + read cost
//! every time. Bounded by both entry count and aggregate bytes; on
//! overflow we evict the least-recently-used entry. The cache is
//! shared across all worker threads via [`MemCache::handle`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use bytes::Bytes;

use super::cache::CacheEntry;

const DEFAULT_MAX_ENTRIES: usize = 256;
const DEFAULT_MAX_BYTES: usize = 64 * 1024 * 1024;

/// Cheap-to-clone shared cached entry: bytes plus the metadata the
/// HTTP layer needs to attach response headers without re-parsing the
/// filename.
#[derive(Debug, Clone)]
pub(crate) struct HotEntry {
    pub(crate) bytes: Bytes,
    pub(crate) max_age: u64,
    pub(crate) expire_at_ms: u128,
    pub(crate) etag: String,
    #[allow(dead_code)]
    pub(crate) extension: &'static str,
}

impl HotEntry {
    pub(crate) fn from_disk(entry: &CacheEntry) -> Self {
        Self {
            bytes: Bytes::from(entry.bytes.clone()),
            max_age: entry.max_age,
            expire_at_ms: entry.expire_at_ms,
            etag: entry.etag.clone(),
            extension: entry.extension,
        }
    }
}

#[derive(Debug)]
struct Inner {
    map: HashMap<String, (Arc<HotEntry>, u64)>,
    order: u64,
    bytes: usize,
    max_entries: usize,
    max_bytes: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct MemCache {
    inner: Arc<Mutex<Inner>>,
}

impl MemCache {
    pub(crate) fn new() -> Self {
        Self::with_limits(DEFAULT_MAX_ENTRIES, DEFAULT_MAX_BYTES)
    }

    pub(crate) fn with_limits(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                map: HashMap::new(),
                order: 0,
                bytes: 0,
                max_entries,
                max_bytes,
            })),
        }
    }

    pub(crate) fn get(&self, key: &str) -> Option<Arc<HotEntry>> {
        let mut g = self.inner.lock().ok()?;
        g.order += 1;
        let next_order = g.order;
        let slot = g.map.get_mut(key)?;
        slot.1 = next_order;
        Some(Arc::clone(&slot.0))
    }

    pub(crate) fn put(&self, key: String, entry: Arc<HotEntry>) {
        let Ok(mut g) = self.inner.lock() else {
            return;
        };
        g.order += 1;
        let next_order = g.order;
        let added = entry.bytes.len();
        if let Some(prev) = g.map.insert(key, (entry, next_order)) {
            g.bytes = g.bytes.saturating_sub(prev.0.bytes.len());
        }
        g.bytes = g.bytes.saturating_add(added);
        evict(&mut g);
    }
}

fn evict(g: &mut Inner) {
    while g.map.len() > g.max_entries || g.bytes > g.max_bytes {
        let Some((victim, _)) = g
            .map
            .iter()
            .min_by_key(|(_, (_, ord))| *ord)
            .map(|(k, (_, ord))| (k.clone(), *ord))
        else {
            break;
        };
        if let Some((evicted, _)) = g.map.remove(&victim) {
            g.bytes = g.bytes.saturating_sub(evicted.bytes.len());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(n: usize) -> Arc<HotEntry> {
        Arc::new(HotEntry {
            bytes: Bytes::from(vec![0u8; n]),
            max_age: 60,
            expire_at_ms: 0,
            etag: "x".into(),
            extension: "webp",
        })
    }

    #[test]
    fn lru_evicts_oldest_on_count_overflow() {
        let cache = MemCache::with_limits(2, 1024);
        cache.put("a".into(), entry(8));
        cache.put("b".into(), entry(8));
        let _ = cache.get("a");
        cache.put("c".into(), entry(8));
        assert!(cache.get("a").is_some());
        assert!(cache.get("b").is_none());
        assert!(cache.get("c").is_some());
    }

    #[test]
    fn lru_evicts_on_byte_overflow() {
        let cache = MemCache::with_limits(8, 16);
        cache.put("a".into(), entry(8));
        cache.put("b".into(), entry(8));
        cache.put("c".into(), entry(8));
        assert!(cache.get("a").is_none());
    }
}
