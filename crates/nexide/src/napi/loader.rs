//! `dlopen` + addon registration entry point.

use std::cell::RefCell;
use std::ffi::OsStr;
use std::path::Path;
use std::sync::OnceLock;

use libloading::{Library, Symbol};
use v8::Global;

use crate::napi::env::{NapiContext, NapiEnv};
use crate::napi::types::{NapiAddonRegisterFn, NapiModule, napi_env, napi_value};

#[derive(Debug, thiserror::Error)]
pub enum NapiLoadError {
    #[error("ERR_DLOPEN_FAILED: '{path}': {source}")]
    Dlopen {
        path: String,
        source: libloading::Error,
    },
    #[error("ERR_NAPI_NO_REGISTER: '{0}' is not an N-API addon")]
    NoEntryPoint(String),
    #[error("ERR_NAPI_INIT_RETURNED_NULL: addon '{0}' init returned no exports")]
    InitReturnedNull(String),
}

thread_local! {
    static LEGACY_PENDING: RefCell<Option<*mut NapiModule>> = const { RefCell::new(None) };
}

pub(crate) fn record_legacy_registration(module: *mut NapiModule) {
    LEGACY_PENDING.with(|slot| {
        slot.replace(Some(module));
    });
}

fn take_legacy_registration() -> Option<*mut NapiModule> {
    LEGACY_PENDING.with(|slot| slot.borrow_mut().take())
}

fn library_cache() -> &'static std::sync::Mutex<std::collections::HashMap<String, &'static Library>>
{
    static CACHE: OnceLock<std::sync::Mutex<std::collections::HashMap<String, &'static Library>>> =
        OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

fn load_or_get(path: &Path) -> Result<&'static Library, NapiLoadError> {
    let key = path.to_string_lossy().into_owned();
    {
        let cache = library_cache().lock().expect("napi library cache poisoned");
        if let Some(lib) = cache.get(&key) {
            return Ok(*lib);
        }
    }
    let lib = unsafe { Library::new(path) }.map_err(|source| NapiLoadError::Dlopen {
        path: key.clone(),
        source,
    })?;
    let leaked: &'static Library = Box::leak(Box::new(lib));
    let mut cache = library_cache().lock().expect("napi library cache poisoned");
    cache.insert(key, leaked);
    Ok(leaked)
}

pub fn load_native_module<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    context: v8::Local<'s, v8::Context>,
    path: &Path,
) -> Result<v8::Local<'s, v8::Value>, NapiLoadError> {
    take_legacy_registration();

    let lib = load_or_get(path)?;
    let modern: Option<Symbol<'_, NapiAddonRegisterFn>> =
        unsafe { lib.get(b"napi_register_module_v1") }.ok();
    let legacy = take_legacy_registration();

    if modern.is_none() && legacy.is_none() {
        return Err(NapiLoadError::NoEntryPoint(path.display().to_string()));
    }

    let exports_obj = v8::Object::new(scope);
    let exports_local: v8::Local<v8::Value> = exports_obj.into();

    let scope_ptr: *mut std::ffi::c_void =
        std::ptr::from_mut::<v8::PinScope<'s, '_>>(scope).cast::<std::ffi::c_void>();
    let context_global = Box::new(Global::new(scope, context));
    let context_ptr: *mut std::ffi::c_void =
        Box::into_raw(context_global).cast::<std::ffi::c_void>();

    let ctx = Box::leak(Box::new(NapiContext::new()));
    let mut env = NapiEnv::new(scope_ptr, context_ptr);
    env.ctx = ctx as *const _;

    let exports_global = Global::new(scope, exports_local);
    let exports_token = env.intern(exports_global);

    let env_ptr: napi_env = std::ptr::from_mut(&mut env);

    let returned: napi_value = if let Some(register_fn) = modern {
        unsafe { register_fn(env_ptr, exports_token) }
    } else if let Some(legacy_desc) = legacy {
        let desc = unsafe { &*legacy_desc };
        let Some(register_fn) = desc.nm_register_func else {
            return Err(NapiLoadError::NoEntryPoint(path.display().to_string()));
        };
        unsafe { register_fn(env_ptr, exports_token) }
    } else {
        unreachable!();
    };

    let _ = unsafe { Box::from_raw(context_ptr.cast::<Global<v8::Context>>()) };

    let returned_local = env.resolve(returned).map(|g| v8::Local::new(scope, &g));
    let final_exports = returned_local.unwrap_or(exports_local);

    if final_exports.is_null_or_undefined() {
        return Err(NapiLoadError::InitReturnedNull(path.display().to_string()));
    }

    Ok(final_exports)
}

#[must_use]
pub fn is_native_addon_path<P: AsRef<Path>>(path: P) -> bool {
    matches!(
        path.as_ref()
            .extension()
            .and_then(OsStr::to_str)
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("node")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_recognition() {
        assert!(is_native_addon_path("/foo/bar.node"));
        assert!(is_native_addon_path("/foo/bar.NODE"));
        assert!(!is_native_addon_path("/foo/bar.js"));
        assert!(!is_native_addon_path("/foo/node"));
    }

    #[test]
    fn legacy_slot_round_trip() {
        assert!(take_legacy_registration().is_none());
        let mut module = NapiModule {
            nm_version: 1,
            nm_flags: 0,
            nm_filename: std::ptr::null(),
            nm_register_func: None,
            nm_modname: std::ptr::null(),
            nm_priv: std::ptr::null_mut(),
            reserved: [std::ptr::null_mut(); 4],
        };
        record_legacy_registration(&mut module);
        assert!(take_legacy_registration().is_some());
        assert!(take_legacy_registration().is_none());
    }
}
