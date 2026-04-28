//! N-API references — strong-only handles addons use to keep JS values
//! alive across async/threadsafe boundaries.
//!
//! Weak refs (count == 0) are not yet implemented; all refs hold the
//! value as a `v8::Global` regardless of count. This is conservative
//! (no premature GC) and matches the most common addon usage where
//! `napi_create_reference(value, 1, &ref)` is always paired with
//! `napi_delete_reference(ref)`.

use std::ffi::c_void;

use v8::Global;

#[repr(C)]
pub struct RefInner {
    pub global: Global<v8::Value>,
    pub count: u32,
}

impl RefInner {
    pub fn boxed(global: Global<v8::Value>, initial_count: u32) -> *mut Self {
        Box::into_raw(Box::new(Self {
            global,
            count: initial_count,
        }))
    }

    /// # Safety
    ///
    /// `ptr` must come from [`Self::boxed`] and not have been freed yet.
    /// Null is accepted and ignored.
    pub unsafe fn drop_raw(ptr: *mut Self) {
        if !ptr.is_null() {
            drop(unsafe { Box::from_raw(ptr) });
        }
    }
}

#[derive(Copy, Clone)]
pub struct RefPtr(pub *mut c_void);

unsafe impl Send for RefPtr {}
unsafe impl Sync for RefPtr {}
