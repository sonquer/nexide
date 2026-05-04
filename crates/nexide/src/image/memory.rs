//! Tiny in-process LRU for optimized image bytes.
//!
//! Sits in front of the on-disk cache so repeated requests for the
//! same `(href, w, q, mime)` triple don't pay the syscall + read cost
//! every time. Bounded by both entry count and aggregate bytes; on
//! overflow we evict the least-recently-used entry. The cache is
//! shared across all worker threads via [`MemCache::handle`].

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use axum::http::HeaderValue;
use bytes::Bytes;
use parking_lot::RwLock;

use super::cache::CacheEntry;
use super::config::ImageConfig;
use super::handler::build_content_disposition;

const DEFAULT_MAX_ENTRIES: usize = 256;
const DEFAULT_MAX_BYTES: usize = 64 * 1024 * 1024;

/// Cheap-to-clone shared cached entry: bytes plus the metadata the
/// HTTP layer needs to attach response headers without re-parsing the
/// filename.
///
/// Carries precomputed `HeaderValue`s for `Cache-Control`, `ETag` and
/// `Content-Disposition` so the hot cache-hit path skips four
/// `format!()` allocations + four `HeaderValue::from_str` validations
/// per request.
#[derive(Debug, Clone)]
pub(crate) struct HotEntry {
    pub(crate) bytes: Bytes,
    pub(crate) expire_at_ms: u128,
    pub(crate) etag: String,
    pub(crate) cache_control_hv: HeaderValue,
    pub(crate) etag_hv: HeaderValue,
    pub(crate) disposition_hv: HeaderValue,
}

impl HotEntry {
    pub(crate) fn from_disk(
        entry: &CacheEntry,
        mime: &'static str,
        url: &str,
        cfg: &ImageConfig,
    ) -> Self {
        let cache_control_hv = HeaderValue::try_from(format!(
            "public, max-age={}, must-revalidate",
            entry.max_age
        ))
        .unwrap_or_else(|_| HeaderValue::from_static("public, must-revalidate"));
        let etag_hv = HeaderValue::try_from(format!("\"{}\"", entry.etag))
            .unwrap_or_else(|_| HeaderValue::from_static("\"\""));
        let disposition_str = build_content_disposition(url, mime, &cfg.content_disposition_type);
        let disposition_hv = HeaderValue::try_from(disposition_str)
            .unwrap_or_else(|_| HeaderValue::from_static("inline"));
        Self {
            bytes: Bytes::from(entry.bytes.clone()),
            expire_at_ms: entry.expire_at_ms,
            etag: entry.etag.clone(),
            cache_control_hv,
            etag_hv,
            disposition_hv,
        }
    }
}

#[derive(Debug)]
struct Slot {
    entry: Arc<HotEntry>,
    order: AtomicU64,
}

#[derive(Debug)]
struct Inner {
    map: RwLock<HashMap<String, Arc<Slot>>>,
    order_counter: AtomicU64,
    bytes: AtomicUsize,
    max_entries: usize,
    max_bytes: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct MemCache {
    inner: Arc<Inner>,
}

impl MemCache {
    pub(crate) fn new() -> Self {
        Self::with_limits(DEFAULT_MAX_ENTRIES, DEFAULT_MAX_BYTES)
    }

    pub(crate) fn with_limits(max_entries: usize, max_bytes: usize) -> Self {
        let inner = Arc::new(Inner {
            map: RwLock::new(HashMap::new()),
            order_counter: AtomicU64::new(0),
            bytes: AtomicUsize::new(0),
            max_entries,
            max_bytes,
        });
        let weak = Arc::downgrade(&inner);
        crate::pool::idle_shrink::register(move || {
            if let Some(strong) = weak.upgrade() {
                let mut g = strong.map.write();
                g.clear();
                strong.bytes.store(0, Ordering::Relaxed);
            }
        });
        Self { inner }
    }

    pub(crate) fn get(&self, key: &str) -> Option<Arc<HotEntry>> {
        let g = match self.inner.map.try_read() {
            Some(g) => {
                crate::diagnostics::contention::record_fast(
                    &crate::diagnostics::contention::MEM_CACHE_FAST,
                );
                g
            }
            None => {
                crate::diagnostics::contention::record_contended(
                    &crate::diagnostics::contention::MEM_CACHE_CONTENDED,
                );
                self.inner.map.read()
            }
        };
        let slot = g.get(key)?;
        let next_order = self.inner.order_counter.fetch_add(1, Ordering::Relaxed) + 1;
        slot.order.store(next_order, Ordering::Relaxed);
        Some(Arc::clone(&slot.entry))
    }

    pub(crate) fn put(&self, key: String, entry: Arc<HotEntry>) {
        let next_order = self.inner.order_counter.fetch_add(1, Ordering::Relaxed) + 1;
        let added = entry.bytes.len();
        let mut g = self.inner.map.write();
        let slot = Arc::new(Slot {
            entry,
            order: AtomicU64::new(next_order),
        });
        if let Some(prev) = g.insert(key, slot) {
            self.inner
                .bytes
                .fetch_sub(prev.entry.bytes.len(), Ordering::Relaxed);
        }
        self.inner.bytes.fetch_add(added, Ordering::Relaxed);
        evict(&self.inner, &mut g);
    }
}

fn evict(inner: &Inner, g: &mut HashMap<String, Arc<Slot>>) {
    loop {
        let bytes = inner.bytes.load(Ordering::Relaxed);
        if g.len() <= inner.max_entries && bytes <= inner.max_bytes {
            break;
        }
        let Some((victim, _)) = g
            .iter()
            .min_by_key(|(_, slot)| slot.order.load(Ordering::Relaxed))
            .map(|(k, slot)| (k.clone(), slot.order.load(Ordering::Relaxed)))
        else {
            break;
        };
        if let Some(evicted) = g.remove(&victim) {
            inner
                .bytes
                .fetch_sub(evicted.entry.bytes.len(), Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(n: usize) -> Arc<HotEntry> {
        Arc::new(HotEntry {
            bytes: Bytes::from(vec![0u8; n]),
            expire_at_ms: 0,
            etag: "x".into(),
            cache_control_hv: HeaderValue::from_static("public, max-age=60, must-revalidate"),
            etag_hv: HeaderValue::from_static("\"x\""),
            disposition_hv: HeaderValue::from_static("inline; filename=\"image.webp\""),
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
