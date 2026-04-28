//! Generic per-isolate handle table.
//!
//! Many host capabilities (TCP sockets, child processes, zlib
//! streams, …) need to hand JavaScript a stable identifier that
//! refers to a Rust-side resource which is not safe to expose
//! directly. The pattern is always the same: mint an integer id,
//! park the resource in a side table keyed by that id, and look it
//! up when subsequent ops fire.
//!
//! [`HandleTable`] formalises that pattern. It is parameterised
//! over the resource type, hands out monotonically increasing
//! `u32` ids, and exposes `with`, `take`, and `remove`
//! helpers so callers do not have to reach into the underlying
//! storage.
//!
//! The table is single-threaded by design - every isolate runs
//! inside a `tokio::task::LocalSet`, so `RefCell` is the correct
//! interior-mutability primitive. `Rc` lets the table be cheaply
//! cloned into op closures without giving up shared ownership.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// Owns a typed resource keyed by a `u32` handle.
///
/// Cloning is cheap: handles share state via `Rc<RefCell<…>>`.
pub(crate) struct HandleTable<T> {
    inner: Rc<RefCell<Inner<T>>>,
}

struct Inner<T> {
    next_id: u32,
    map: HashMap<u32, T>,
}

impl<T> Default for HandleTable<T> {
    fn default() -> Self {
        Self {
            inner: Rc::new(RefCell::new(Inner {
                next_id: 1,
                map: HashMap::new(),
            })),
        }
    }
}

impl<T> Clone for HandleTable<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Rc::clone(&self.inner),
        }
    }
}

impl<T> HandleTable<T> {
    /// Inserts `value` into the table and returns its freshly
    /// minted handle id.
    pub(crate) fn insert(&self, value: T) -> u32 {
        let mut inner = self.inner.borrow_mut();
        let id = inner.next_id;
        inner.next_id = inner.next_id.wrapping_add(1).max(1);
        inner.map.insert(id, value);
        id
    }

    /// Removes the resource referenced by `id`, returning it to the
    /// caller for cleanup.
    pub(crate) fn take(&self, id: u32) -> Option<T> {
        self.inner.borrow_mut().map.remove(&id)
    }

    /// Returns `true` if `id` refers to a live resource and forgets
    /// the entry. Equivalent to `take(id).is_some()` but avoids
    /// returning the value when the caller just wants to close.
    pub(crate) fn remove(&self, id: u32) -> bool {
        self.inner.borrow_mut().map.remove(&id).is_some()
    }

    /// Runs `f` against the resource referenced by `id`. Returns
    /// `None` if no such handle exists.
    pub(crate) fn with<R>(&self, id: u32, f: impl FnOnce(&T) -> R) -> Option<R> {
        self.inner.borrow().map.get(&id).map(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_lookup_round_trip() {
        let table: HandleTable<String> = HandleTable::default();
        let id = table.insert("hello".to_owned());
        assert_eq!(table.with(id, |v| v.clone()).as_deref(), Some("hello"));
    }

    #[test]
    fn take_removes_entry() {
        let table: HandleTable<u32> = HandleTable::default();
        let id = table.insert(42);
        assert_eq!(table.take(id), Some(42));
        assert!(table.with(id, |_| ()).is_none());
    }

    #[test]
    fn ids_are_monotonic() {
        let table: HandleTable<u8> = HandleTable::default();
        let a = table.insert(0);
        let b = table.insert(0);
        assert!(b > a);
    }
}
