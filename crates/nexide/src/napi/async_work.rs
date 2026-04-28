//! Async-work plumbing: maps N-API's
//! `napi_create_async_work` / `queue_async_work` onto tokio's blocking
//! pool and the per-isolate `napi_work_tx` channel.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::napi::types::{NapiAsyncComplete, NapiAsyncExecute};

#[repr(C)]
pub struct AsyncWorkInner {
    pub execute: NapiAsyncExecute,
    pub complete: Option<NapiAsyncComplete>,
    pub data: SyncPtr,
    pub cancelled: AtomicBool,
    pub queued: AtomicBool,
}

impl AsyncWorkInner {
    pub fn boxed(
        execute: NapiAsyncExecute,
        complete: Option<NapiAsyncComplete>,
        data: *mut c_void,
    ) -> *mut Self {
        Box::into_raw(Box::new(Self {
            execute,
            complete,
            data: SyncPtr(data),
            cancelled: AtomicBool::new(false),
            queued: AtomicBool::new(false),
        }))
    }

    /// # Safety
    ///
    /// `ptr` must come from [`Self::boxed`] and not have been freed yet. Null
    /// is accepted and ignored.
    pub unsafe fn drop_raw(ptr: *mut Self) {
        if !ptr.is_null() {
            drop(unsafe { Box::from_raw(ptr) });
        }
    }
}

#[derive(Copy, Clone)]
pub struct SyncPtr(pub *mut c_void);

unsafe impl Send for SyncPtr {}
unsafe impl Sync for SyncPtr {}

#[derive(Copy, Clone)]
pub struct InnerPtr(pub *mut AsyncWorkInner);

unsafe impl Send for InnerPtr {}
unsafe impl Sync for InnerPtr {}

/// # Safety
///
/// `inner` must be a pointer returned by [`AsyncWorkInner::boxed`] that has not
/// yet been freed via [`AsyncWorkInner::drop_raw`]. Passing a null pointer is a
/// no-op.
pub unsafe fn cancel(inner: *mut AsyncWorkInner) {
    if inner.is_null() {
        return;
    }
    unsafe { (*inner).cancelled.store(true, Ordering::SeqCst) };
}
