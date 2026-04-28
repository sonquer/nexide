//! Threadsafe-function plumbing.
//!
//! N-API threadsafe-functions let any thread (typically a worker pool
//! owned by the addon — Prisma's QueryEngine, libuv-style native pools,
//! tokio runtimes embedded in addons) push a callback that runs on the
//! V8 isolate thread. We re-use the per-isolate `napi_work_tx` channel
//! (set up for `napi_queue_async_work`); each call repacks the user
//! data into a closure that the engine drains under a real `PinScope`.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicUsize};

use tokio::sync::mpsc::UnboundedSender;
use v8::Global;

use crate::engine::v8_engine::NapiWorkItem;
use crate::napi::types::{NapiFinalize, NapiThreadsafeFunctionCallJs};

#[repr(C)]
pub struct TsfnInner {
    pub js_callback: Option<Global<v8::Value>>,
    pub call_js: Option<NapiThreadsafeFunctionCallJs>,
    pub context: *mut c_void,
    pub finalize: Option<NapiFinalize>,
    pub finalize_data: *mut c_void,
    /// Live thread acquisitions. Initial = `initial_thread_count`. Each
    /// `napi_acquire_threadsafe_function` adds 1; each release subs 1.
    /// When it hits 0 the tsfn is finalised on the JS thread.
    pub thread_count: AtomicUsize,
    /// Set once `Abort` has been requested; subsequent calls fail with
    /// `Closing` and queued items are dropped on the JS side.
    pub aborted: AtomicBool,
    /// Refcounting flag for libuv-style "keep the loop alive". Nexide's
    /// loop is driven by the engine pump; we record the toggle so
    /// addons that ref/unref aren't surprised, but it has no effect on
    /// shutdown today.
    pub kept_alive: AtomicBool,
    /// Pending items queued towards the JS thread. Kept for parity with
    /// Node's max_queue_size (0 = unbounded).
    pub queued: AtomicIsize,
    pub work_tx: UnboundedSender<NapiWorkItem>,
}

impl TsfnInner {
    pub fn boxed(inner: TsfnInner) -> *mut Self {
        Box::into_raw(Box::new(inner))
    }

    /// # Safety
    ///
    /// `ptr` must come from [`Self::boxed`] and not have been freed.
    pub unsafe fn drop_raw(ptr: *mut Self) {
        if !ptr.is_null() {
            drop(unsafe { Box::from_raw(ptr) });
        }
    }
}

/// Send-wrapper around `*mut TsfnInner` for crossing thread/closure
/// boundaries. The pointer is logically owned by the JS thread; other
/// threads only reach the atomics and the (cloned) sender.
#[derive(Copy, Clone)]
pub struct TsfnPtr(pub *mut TsfnInner);

unsafe impl Send for TsfnPtr {}
unsafe impl Sync for TsfnPtr {}
