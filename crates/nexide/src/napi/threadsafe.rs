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
    pub thread_count: AtomicUsize,
    pub aborted: AtomicBool,
    pub kept_alive: AtomicBool,
    pub queued: AtomicIsize,
    pub work_tx: UnboundedSender<NapiWorkItem>,
    pub wakeup: std::sync::Arc<tokio::sync::Notify>,
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
