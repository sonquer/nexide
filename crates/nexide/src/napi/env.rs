//! Per-FFI-call binding handed to addons as the opaque `napi_env*`.

use std::cell::RefCell;
use std::ffi::c_void;

use v8::Global;

use crate::napi::types::napi_value;

/// Per-addon-instance data stored via `napi_set_instance_data`.
pub struct InstanceData {
    pub data: *mut c_void,
    pub finalize:
        Option<unsafe extern "C" fn(env: *mut c_void, data: *mut c_void, hint: *mut c_void)>,
    pub finalize_hint: *mut c_void,
}

/// One env-cleanup hook registered via `napi_add_env_cleanup_hook`.
#[derive(Clone, Copy)]
pub struct CleanupHook {
    pub fun: Option<unsafe extern "C" fn(*mut c_void)>,
    pub arg: *mut c_void,
}

pub struct NapiContext {
    pub module_name: Option<String>,
    pub refs: RefCell<Vec<Box<Global<v8::Value>>>>,
    pub instance_data: RefCell<Option<InstanceData>>,
    pub cleanup_hooks: RefCell<Vec<CleanupHook>>,
}

impl Default for NapiContext {
    fn default() -> Self {
        Self::new()
    }
}

impl NapiContext {
    #[must_use]
    pub fn new() -> Self {
        Self {
            module_name: None,
            refs: RefCell::new(Vec::new()),
            instance_data: RefCell::new(None),
            cleanup_hooks: RefCell::new(Vec::new()),
        }
    }
}

pub struct NapiEnv {
    pub scope: *mut std::ffi::c_void,
    pub context: *mut std::ffi::c_void,
    pub handles: RefCell<Vec<Global<v8::Value>>>,
    pub pending_exception: RefCell<Option<Global<v8::Value>>>,
    pub ctx: *const NapiContext,
}

impl NapiEnv {
    #[must_use]
    pub fn new(scope: *mut std::ffi::c_void, context: *mut std::ffi::c_void) -> Self {
        Self {
            scope,
            context,
            handles: RefCell::new(Vec::with_capacity(16)),
            pending_exception: RefCell::new(None),
            ctx: std::ptr::null(),
        }
    }

    pub fn intern(&self, value: Global<v8::Value>) -> napi_value {
        let mut handles = self.handles.borrow_mut();
        let idx = handles.len();
        handles.push(value);
        napi_value((idx + 1) as *mut std::ffi::c_void)
    }

    #[must_use]
    pub fn resolve(&self, token: napi_value) -> Option<Global<v8::Value>> {
        let idx = (token.0 as usize).checked_sub(1)?;
        self.handles.borrow().get(idx).cloned()
    }
}
