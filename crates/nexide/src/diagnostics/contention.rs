//! Atomic counters for hot-path lock contention.

use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) static PRERENDER_READ_FAST: AtomicU64 = AtomicU64::new(0);
pub(crate) static PRERENDER_READ_CONTENDED: AtomicU64 = AtomicU64::new(0);
pub(crate) static PRERENDER_WRITE_FAST: AtomicU64 = AtomicU64::new(0);
pub(crate) static PRERENDER_WRITE_CONTENDED: AtomicU64 = AtomicU64::new(0);

pub(crate) static RAM_CACHE_FAST: AtomicU64 = AtomicU64::new(0);
pub(crate) static RAM_CACHE_CONTENDED: AtomicU64 = AtomicU64::new(0);

pub(crate) static MEM_CACHE_FAST: AtomicU64 = AtomicU64::new(0);
pub(crate) static MEM_CACHE_CONTENDED: AtomicU64 = AtomicU64::new(0);

pub(crate) static INFLIGHT_ACQUIRE_FAST: AtomicU64 = AtomicU64::new(0);
pub(crate) static INFLIGHT_ACQUIRE_CONTENDED: AtomicU64 = AtomicU64::new(0);

#[inline]
pub(crate) fn record_fast(c: &AtomicU64) {
    c.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub(crate) fn record_contended(c: &AtomicU64) {
    c.fetch_add(1, Ordering::Relaxed);
}

#[derive(Clone, Copy, Default, Debug)]
pub(crate) struct Snapshot {
    pub(crate) prerender_read_fast: u64,
    pub(crate) prerender_read_contended: u64,
    pub(crate) prerender_write_fast: u64,
    pub(crate) prerender_write_contended: u64,
    pub(crate) ram_cache_fast: u64,
    pub(crate) ram_cache_contended: u64,
    pub(crate) mem_cache_fast: u64,
    pub(crate) mem_cache_contended: u64,
    pub(crate) inflight_acquire_fast: u64,
    pub(crate) inflight_acquire_contended: u64,
}

impl Snapshot {
    pub(crate) fn current() -> Self {
        Self {
            prerender_read_fast: PRERENDER_READ_FAST.load(Ordering::Relaxed),
            prerender_read_contended: PRERENDER_READ_CONTENDED.load(Ordering::Relaxed),
            prerender_write_fast: PRERENDER_WRITE_FAST.load(Ordering::Relaxed),
            prerender_write_contended: PRERENDER_WRITE_CONTENDED.load(Ordering::Relaxed),
            ram_cache_fast: RAM_CACHE_FAST.load(Ordering::Relaxed),
            ram_cache_contended: RAM_CACHE_CONTENDED.load(Ordering::Relaxed),
            mem_cache_fast: MEM_CACHE_FAST.load(Ordering::Relaxed),
            mem_cache_contended: MEM_CACHE_CONTENDED.load(Ordering::Relaxed),
            inflight_acquire_fast: INFLIGHT_ACQUIRE_FAST.load(Ordering::Relaxed),
            inflight_acquire_contended: INFLIGHT_ACQUIRE_CONTENDED.load(Ordering::Relaxed),
        }
    }

    pub(crate) fn delta(&self, prev: &Self) -> Self {
        Self {
            prerender_read_fast: self.prerender_read_fast.saturating_sub(prev.prerender_read_fast),
            prerender_read_contended: self
                .prerender_read_contended
                .saturating_sub(prev.prerender_read_contended),
            prerender_write_fast: self
                .prerender_write_fast
                .saturating_sub(prev.prerender_write_fast),
            prerender_write_contended: self
                .prerender_write_contended
                .saturating_sub(prev.prerender_write_contended),
            ram_cache_fast: self.ram_cache_fast.saturating_sub(prev.ram_cache_fast),
            ram_cache_contended: self
                .ram_cache_contended
                .saturating_sub(prev.ram_cache_contended),
            mem_cache_fast: self.mem_cache_fast.saturating_sub(prev.mem_cache_fast),
            mem_cache_contended: self
                .mem_cache_contended
                .saturating_sub(prev.mem_cache_contended),
            inflight_acquire_fast: self
                .inflight_acquire_fast
                .saturating_sub(prev.inflight_acquire_fast),
            inflight_acquire_contended: self
                .inflight_acquire_contended
                .saturating_sub(prev.inflight_acquire_contended),
        }
    }

    pub(crate) fn has_activity(&self) -> bool {
        self.prerender_read_fast
            | self.prerender_read_contended
            | self.prerender_write_fast
            | self.prerender_write_contended
            | self.ram_cache_fast
            | self.ram_cache_contended
            | self.mem_cache_fast
            | self.mem_cache_contended
            | self.inflight_acquire_fast
            | self.inflight_acquire_contended
            != 0
    }
}

#[cfg(test)]
pub(crate) fn reset_for_tests() {
    for c in [
        &PRERENDER_READ_FAST,
        &PRERENDER_READ_CONTENDED,
        &PRERENDER_WRITE_FAST,
        &PRERENDER_WRITE_CONTENDED,
        &RAM_CACHE_FAST,
        &RAM_CACHE_CONTENDED,
        &MEM_CACHE_FAST,
        &MEM_CACHE_CONTENDED,
        &INFLIGHT_ACQUIRE_FAST,
        &INFLIGHT_ACQUIRE_CONTENDED,
    ] {
        c.store(0, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_delta_subtracts_field_by_field() {
        reset_for_tests();
        record_fast(&PRERENDER_READ_FAST);
        record_fast(&PRERENDER_READ_FAST);
        record_contended(&MEM_CACHE_CONTENDED);
        let s1 = Snapshot::current();
        record_fast(&PRERENDER_READ_FAST);
        record_contended(&MEM_CACHE_CONTENDED);
        let s2 = Snapshot::current();
        let d = s2.delta(&s1);
        assert_eq!(d.prerender_read_fast, 1);
        assert_eq!(d.mem_cache_contended, 1);
        assert_eq!(d.ram_cache_fast, 0);
    }

    #[test]
    fn has_activity_detects_any_nonzero_field() {
        let mut s = Snapshot::default();
        assert!(!s.has_activity());
        s.ram_cache_fast = 1;
        assert!(s.has_activity());
    }
}
