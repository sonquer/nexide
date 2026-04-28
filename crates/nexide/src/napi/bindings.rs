//! `extern "C"` implementations of the N-API surface.

#![allow(
    non_snake_case,
    clippy::missing_safety_doc,
    clippy::not_unsafe_ptr_arg_deref
)]

use std::ffi::{c_char, c_void};
use std::ptr;

use v8::Global;

use crate::engine::v8_engine::{NapiWorkItem, napi_work_sender};
use crate::napi::async_work::AsyncWorkInner;
use crate::napi::callbacks::{CallbackBundle, CallbackInfo, trampoline};
use crate::napi::env::NapiEnv;
use crate::napi::references::RefInner;
use crate::napi::threadsafe::TsfnInner;
use crate::napi::types::{
    NapiAsyncComplete, NapiAsyncExecute, NapiCallback, NapiFinalize, NapiStatus,
    NapiThreadsafeFunctionCallJs, NapiThreadsafeFunctionCallMode,
    NapiThreadsafeFunctionReleaseMode, NapiTypedArrayType, NapiValueType, napi_async_work,
    napi_callback_info, napi_env, napi_ref, napi_threadsafe_function, napi_value,
};

unsafe fn scope_from_env<'s, 'i>(env: &NapiEnv) -> &'s mut v8::PinScope<'s, 'i> {
    debug_assert!(!env.scope.is_null());
    unsafe { &mut *env.scope.cast::<v8::PinScope<'s, 'i>>() }
}

unsafe fn context_from_env<'s>(env: &NapiEnv) -> v8::Local<'s, v8::Context> {
    debug_assert!(!env.context.is_null());
    let global_ref = unsafe { &*env.context.cast::<Global<v8::Context>>() };
    let scope = unsafe { scope_from_env(env) };
    v8::Local::new(scope, global_ref)
}

unsafe fn env_ref<'a>(env: napi_env) -> Option<&'a NapiEnv> {
    if env.is_null() {
        return None;
    }
    Some(unsafe { &*env })
}

unsafe fn write_value_out(
    env: &NapiEnv,
    out: *mut napi_value,
    value: v8::Local<v8::Value>,
    scope: &mut v8::PinScope<'_, '_>,
) -> NapiStatus {
    if out.is_null() {
        return NapiStatus::InvalidArg;
    }
    let global = Global::new(scope, value);
    let token = env.intern(global);
    unsafe { ptr::write(out, token) };
    NapiStatus::Ok
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_get_undefined(env: napi_env, result: *mut napi_value) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    let scope = unsafe { scope_from_env(env) };
    let v: v8::Local<v8::Value> = v8::undefined(scope).into();
    unsafe { write_value_out(env, result, v, scope) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_get_null(env: napi_env, result: *mut napi_value) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    let scope = unsafe { scope_from_env(env) };
    let v: v8::Local<v8::Value> = v8::null(scope).into();
    unsafe { write_value_out(env, result, v, scope) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_get_boolean(
    env: napi_env,
    value: bool,
    result: *mut napi_value,
) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    let scope = unsafe { scope_from_env(env) };
    let v: v8::Local<v8::Value> = v8::Boolean::new(scope, value).into();
    unsafe { write_value_out(env, result, v, scope) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_create_double(
    env: napi_env,
    value: f64,
    result: *mut napi_value,
) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    let scope = unsafe { scope_from_env(env) };
    let v: v8::Local<v8::Value> = v8::Number::new(scope, value).into();
    unsafe { write_value_out(env, result, v, scope) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_create_int32(
    env: napi_env,
    value: i32,
    result: *mut napi_value,
) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    let scope = unsafe { scope_from_env(env) };
    let v: v8::Local<v8::Value> = v8::Integer::new(scope, value).into();
    unsafe { write_value_out(env, result, v, scope) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_create_uint32(
    env: napi_env,
    value: u32,
    result: *mut napi_value,
) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    let scope = unsafe { scope_from_env(env) };
    let v: v8::Local<v8::Value> = v8::Integer::new_from_unsigned(scope, value).into();
    unsafe { write_value_out(env, result, v, scope) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_create_int64(
    env: napi_env,
    value: i64,
    result: *mut napi_value,
) -> NapiStatus {
    #[allow(clippy::cast_precision_loss)]
    let f = value as f64;
    unsafe { napi_create_double(env, f, result) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_create_string_utf8(
    env: napi_env,
    str_ptr: *const c_char,
    length: usize,
    result: *mut napi_value,
) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    if str_ptr.is_null() && length != 0 {
        return NapiStatus::InvalidArg;
    }
    let bytes: &[u8] = if length == usize::MAX {
        if str_ptr.is_null() {
            &[]
        } else {
            unsafe { std::ffi::CStr::from_ptr(str_ptr) }.to_bytes()
        }
    } else if length == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(str_ptr.cast::<u8>(), length) }
    };
    let Ok(text) = std::str::from_utf8(bytes) else {
        return NapiStatus::InvalidArg;
    };
    let scope = unsafe { scope_from_env(env) };
    let Some(s) = v8::String::new(scope, text) else {
        return NapiStatus::GenericFailure;
    };
    let v: v8::Local<v8::Value> = s.into();
    unsafe { write_value_out(env, result, v, scope) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_get_value_string_utf8(
    env: napi_env,
    value: napi_value,
    buf: *mut c_char,
    bufsize: usize,
    result: *mut usize,
) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    let Some(global) = env.resolve(value) else {
        return NapiStatus::InvalidArg;
    };
    let scope = unsafe { scope_from_env(env) };
    let local = v8::Local::new(scope, &global);
    let Ok(s) = v8::Local::<v8::String>::try_from(local) else {
        return NapiStatus::StringExpected;
    };
    let utf8 = s.to_rust_string_lossy(scope);
    let bytes = utf8.as_bytes();

    if buf.is_null() {
        if !result.is_null() {
            unsafe { ptr::write(result, bytes.len()) };
        }
        return NapiStatus::Ok;
    }
    if bufsize == 0 {
        if !result.is_null() {
            unsafe { ptr::write(result, 0) };
        }
        return NapiStatus::Ok;
    }
    let max = bufsize - 1;
    let copy = bytes.len().min(max);
    unsafe { ptr::copy_nonoverlapping(bytes.as_ptr().cast::<c_char>(), buf, copy) };
    unsafe { ptr::write(buf.add(copy), 0) };
    if !result.is_null() {
        unsafe { ptr::write(result, copy) };
    }
    NapiStatus::Ok
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_typeof(
    env: napi_env,
    value: napi_value,
    result: *mut NapiValueType,
) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    let Some(global) = env.resolve(value) else {
        return NapiStatus::InvalidArg;
    };
    let scope = unsafe { scope_from_env(env) };
    let local = v8::Local::new(scope, &global);
    let kind = if local.is_undefined() {
        NapiValueType::Undefined
    } else if local.is_null() {
        NapiValueType::Null
    } else if local.is_boolean() {
        NapiValueType::Boolean
    } else if local.is_number() {
        NapiValueType::Number
    } else if local.is_string() {
        NapiValueType::String
    } else if local.is_symbol() {
        NapiValueType::Symbol
    } else if local.is_function() {
        NapiValueType::Function
    } else if local.is_big_int() {
        NapiValueType::BigInt
    } else if local.is_object() {
        NapiValueType::Object
    } else {
        NapiValueType::Undefined
    };
    if result.is_null() {
        return NapiStatus::InvalidArg;
    }
    unsafe { ptr::write(result, kind) };
    NapiStatus::Ok
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_get_value_double(
    env: napi_env,
    value: napi_value,
    result: *mut f64,
) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    let Some(global) = env.resolve(value) else {
        return NapiStatus::InvalidArg;
    };
    let scope = unsafe { scope_from_env(env) };
    let local = v8::Local::new(scope, &global);
    let Ok(num) = v8::Local::<v8::Number>::try_from(local) else {
        return NapiStatus::NumberExpected;
    };
    if result.is_null() {
        return NapiStatus::InvalidArg;
    }
    unsafe { ptr::write(result, num.value()) };
    NapiStatus::Ok
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_get_value_bool(
    env: napi_env,
    value: napi_value,
    result: *mut bool,
) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    let Some(global) = env.resolve(value) else {
        return NapiStatus::InvalidArg;
    };
    let scope = unsafe { scope_from_env(env) };
    let local = v8::Local::new(scope, &global);
    let Ok(b) = v8::Local::<v8::Boolean>::try_from(local) else {
        return NapiStatus::BooleanExpected;
    };
    if result.is_null() {
        return NapiStatus::InvalidArg;
    }
    unsafe { ptr::write(result, b.is_true()) };
    NapiStatus::Ok
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_create_object(env: napi_env, result: *mut napi_value) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    let scope = unsafe { scope_from_env(env) };
    let v: v8::Local<v8::Value> = v8::Object::new(scope).into();
    unsafe { write_value_out(env, result, v, scope) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_create_array(env: napi_env, result: *mut napi_value) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    let scope = unsafe { scope_from_env(env) };
    let v: v8::Local<v8::Value> = v8::Array::new(scope, 0).into();
    unsafe { write_value_out(env, result, v, scope) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_set_named_property(
    env: napi_env,
    object: napi_value,
    utf8name: *const c_char,
    value: napi_value,
) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    if utf8name.is_null() {
        return NapiStatus::InvalidArg;
    }
    let Some(obj_global) = env.resolve(object) else {
        return NapiStatus::InvalidArg;
    };
    let Some(val_global) = env.resolve(value) else {
        return NapiStatus::InvalidArg;
    };

    let name_bytes = unsafe { std::ffi::CStr::from_ptr(utf8name) }.to_bytes();
    let Ok(name) = std::str::from_utf8(name_bytes) else {
        return NapiStatus::InvalidArg;
    };

    let scope = unsafe { scope_from_env(env) };
    let obj_local = v8::Local::new(scope, &obj_global);
    let Ok(obj) = v8::Local::<v8::Object>::try_from(obj_local) else {
        return NapiStatus::ObjectExpected;
    };
    let Some(key) = v8::String::new(scope, name) else {
        return NapiStatus::GenericFailure;
    };
    let val_local = v8::Local::new(scope, &val_global);
    if obj.set(scope, key.into(), val_local).is_some() {
        NapiStatus::Ok
    } else {
        NapiStatus::GenericFailure
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_get_named_property(
    env: napi_env,
    object: napi_value,
    utf8name: *const c_char,
    result: *mut napi_value,
) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    if utf8name.is_null() {
        return NapiStatus::InvalidArg;
    }
    let Some(obj_global) = env.resolve(object) else {
        return NapiStatus::InvalidArg;
    };
    let name_bytes = unsafe { std::ffi::CStr::from_ptr(utf8name) }.to_bytes();
    let Ok(name) = std::str::from_utf8(name_bytes) else {
        return NapiStatus::InvalidArg;
    };
    let scope = unsafe { scope_from_env(env) };
    let obj_local = v8::Local::new(scope, &obj_global);
    let Ok(obj) = v8::Local::<v8::Object>::try_from(obj_local) else {
        return NapiStatus::ObjectExpected;
    };
    let Some(key) = v8::String::new(scope, name) else {
        return NapiStatus::GenericFailure;
    };
    let Some(value) = obj.get(scope, key.into()) else {
        return NapiStatus::GenericFailure;
    };
    unsafe { write_value_out(env, result, value, scope) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_get_global(env: napi_env, result: *mut napi_value) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    let context_local = unsafe { context_from_env(env) };
    let scope = unsafe { scope_from_env(env) };
    let global = context_local.global(scope);
    let v: v8::Local<v8::Value> = global.into();
    unsafe { write_value_out(env, result, v, scope) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_throw(env: napi_env, error: napi_value) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    let Some(err_global) = env.resolve(error) else {
        return NapiStatus::InvalidArg;
    };
    let scope = unsafe { scope_from_env(env) };
    let err_local = v8::Local::new(scope, &err_global);
    scope.throw_exception(err_local);
    *env.pending_exception.borrow_mut() = Some(Global::new(scope, err_local));
    NapiStatus::Ok
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_throw_error(
    env: napi_env,
    code: *const c_char,
    msg: *const c_char,
) -> NapiStatus {
    unsafe { throw_named(env, code, msg, ErrKind::Error) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_throw_type_error(
    env: napi_env,
    code: *const c_char,
    msg: *const c_char,
) -> NapiStatus {
    unsafe { throw_named(env, code, msg, ErrKind::Type) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_throw_range_error(
    env: napi_env,
    code: *const c_char,
    msg: *const c_char,
) -> NapiStatus {
    unsafe { throw_named(env, code, msg, ErrKind::Range) }
}

#[derive(Copy, Clone)]
enum ErrKind {
    Error,
    Type,
    Range,
}

unsafe fn throw_named(
    env: napi_env,
    code: *const c_char,
    msg: *const c_char,
    kind: ErrKind,
) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    if msg.is_null() {
        return NapiStatus::InvalidArg;
    }
    let msg_bytes = unsafe { std::ffi::CStr::from_ptr(msg) }.to_bytes();
    let Ok(msg_str) = std::str::from_utf8(msg_bytes) else {
        return NapiStatus::InvalidArg;
    };
    let scope = unsafe { scope_from_env(env) };
    let Some(msg_v8) = v8::String::new(scope, msg_str) else {
        return NapiStatus::GenericFailure;
    };
    let err: v8::Local<v8::Value> = match kind {
        ErrKind::Error => v8::Exception::error(scope, msg_v8),
        ErrKind::Type => v8::Exception::type_error(scope, msg_v8),
        ErrKind::Range => v8::Exception::range_error(scope, msg_v8),
    };
    if !code.is_null() {
        let code_bytes = unsafe { std::ffi::CStr::from_ptr(code) }.to_bytes();
        if let Ok(code_str) = std::str::from_utf8(code_bytes)
            && let Ok(err_obj) = v8::Local::<v8::Object>::try_from(err)
            && let (Some(key), Some(value)) = (
                v8::String::new(scope, "code"),
                v8::String::new(scope, code_str),
            )
        {
            let key_v: v8::Local<v8::Value> = key.into();
            let val_v: v8::Local<v8::Value> = value.into();
            let _ = err_obj.set(scope, key_v, val_v);
        }
    }
    scope.throw_exception(err);
    *env.pending_exception.borrow_mut() = Some(Global::new(scope, err));
    NapiStatus::Ok
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_is_exception_pending(env: napi_env, result: *mut bool) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    if result.is_null() {
        return NapiStatus::InvalidArg;
    }
    let pending = env.pending_exception.borrow().is_some();
    unsafe { ptr::write(result, pending) };
    NapiStatus::Ok
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_get_version(env: napi_env, result: *mut u32) -> NapiStatus {
    if env.is_null() || result.is_null() {
        return NapiStatus::InvalidArg;
    }
    unsafe { ptr::write(result, 9) };
    NapiStatus::Ok
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_module_register(module: *mut crate::napi::types::NapiModule) {
    crate::napi::loader::record_legacy_registration(module);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_set_instance_data(
    _env: napi_env,
    _data: *mut c_void,
    _finalize_cb: *mut c_void,
    _finalize_hint: *mut c_void,
) -> NapiStatus {
    NapiStatus::Ok
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_get_instance_data(
    _env: napi_env,
    data: *mut *mut c_void,
) -> NapiStatus {
    if data.is_null() {
        return NapiStatus::InvalidArg;
    }
    unsafe { ptr::write(data, ptr::null_mut()) };
    NapiStatus::Ok
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_create_function(
    env: napi_env,
    utf8name: *const c_char,
    length: usize,
    cb: Option<NapiCallback>,
    data: *mut c_void,
    result: *mut napi_value,
) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    let Some(cb) = cb else {
        return NapiStatus::InvalidArg;
    };
    let scope = unsafe { scope_from_env(env) };

    let bundle = Box::new(CallbackBundle { callback: cb, data });
    let bundle_ptr = Box::into_raw(bundle).cast::<c_void>();
    let external = v8::External::new(scope, bundle_ptr);

    let Some(function) = v8::Function::builder(trampoline)
        .data(external.into())
        .build(scope)
    else {
        drop(unsafe { Box::from_raw(bundle_ptr.cast::<CallbackBundle>()) });
        return NapiStatus::GenericFailure;
    };

    if !utf8name.is_null() && length != 0 {
        let bytes = if length == usize::MAX {
            unsafe { std::ffi::CStr::from_ptr(utf8name) }.to_bytes()
        } else {
            unsafe { std::slice::from_raw_parts(utf8name.cast::<u8>(), length) }
        };
        if let Ok(name) = std::str::from_utf8(bytes)
            && let Some(name_v8) = v8::String::new(scope, name)
        {
            function.set_name(name_v8);
        }
    }

    let v: v8::Local<v8::Value> = function.into();
    unsafe { write_value_out(env, result, v, scope) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_get_cb_info(
    env: napi_env,
    cbinfo: napi_callback_info,
    argc: *mut usize,
    argv: *mut napi_value,
    this_arg: *mut napi_value,
    data: *mut *mut c_void,
) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    if cbinfo.0.is_null() {
        return NapiStatus::InvalidArg;
    }
    let info: &CallbackInfo<'_, '_> = unsafe { &*cbinfo.0.cast::<CallbackInfo<'_, '_>>() };
    let scope = unsafe { scope_from_env(env) };

    let real_argc = info.args.length() as usize;

    if !argv.is_null() && !argc.is_null() {
        let requested = unsafe { *argc };
        let copy = real_argc.min(requested);
        let undef: v8::Local<v8::Value> = v8::undefined(scope).into();
        let undef_global = Global::new(scope, undef);
        let undef_token = env.intern(undef_global);
        for i in 0..requested {
            let value = if i < copy {
                let v = info.args.get(i as i32);
                let g = Global::new(scope, v);
                env.intern(g)
            } else {
                undef_token
            };
            unsafe { ptr::write(argv.add(i), value) };
        }
    }
    if !argc.is_null() {
        unsafe { ptr::write(argc, real_argc) };
    }
    if !this_arg.is_null() {
        let g = Global::new(scope, info.this);
        let token = env.intern(g);
        unsafe { ptr::write(this_arg, token) };
    }
    if !data.is_null() {
        unsafe { ptr::write(data, info.data) };
    }
    NapiStatus::Ok
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_get_new_target(
    env: napi_env,
    cbinfo: napi_callback_info,
    result: *mut napi_value,
) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    if cbinfo.0.is_null() || result.is_null() {
        return NapiStatus::InvalidArg;
    }
    let info: &CallbackInfo<'_, '_> = unsafe { &*cbinfo.0.cast::<CallbackInfo<'_, '_>>() };
    let scope = unsafe { scope_from_env(env) };
    match info.new_target {
        Some(target) => {
            let g = Global::new(scope, target);
            unsafe { ptr::write(result, env.intern(g)) };
        }
        None => unsafe { ptr::write(result, napi_value(ptr::null_mut())) },
    }
    NapiStatus::Ok
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_call_function(
    env: napi_env,
    recv: napi_value,
    func: napi_value,
    argc: usize,
    argv: *const napi_value,
    result: *mut napi_value,
) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    let Some(recv_g) = env.resolve(recv) else {
        return NapiStatus::InvalidArg;
    };
    let Some(func_g) = env.resolve(func) else {
        return NapiStatus::InvalidArg;
    };
    let scope = unsafe { scope_from_env(env) };
    let func_local = v8::Local::new(scope, &func_g);
    let Ok(function) = v8::Local::<v8::Function>::try_from(func_local) else {
        return NapiStatus::FunctionExpected;
    };
    let recv_local = v8::Local::new(scope, &recv_g);

    let mut args_vec: Vec<v8::Local<v8::Value>> = Vec::with_capacity(argc);
    if argc > 0 {
        if argv.is_null() {
            return NapiStatus::InvalidArg;
        }
        for i in 0..argc {
            let token = unsafe { *argv.add(i) };
            let Some(g) = env.resolve(token) else {
                return NapiStatus::InvalidArg;
            };
            args_vec.push(v8::Local::new(scope, &g));
        }
    }

    v8::tc_scope!(let try_catch, scope);
    let returned = function.call(try_catch, recv_local, &args_vec);
    if let Some(exc) = try_catch.exception() {
        *env.pending_exception.borrow_mut() = Some(Global::new(try_catch, exc));
        try_catch.rethrow();
        return NapiStatus::PendingException;
    }
    let value = returned.unwrap_or_else(|| v8::undefined(try_catch).into());
    if !result.is_null() {
        unsafe { write_value_out(env, result, value, try_catch) };
    }
    NapiStatus::Ok
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_new_instance(
    env: napi_env,
    constructor: napi_value,
    argc: usize,
    argv: *const napi_value,
    result: *mut napi_value,
) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    let Some(ctor_g) = env.resolve(constructor) else {
        return NapiStatus::InvalidArg;
    };
    let scope = unsafe { scope_from_env(env) };
    let ctor_local = v8::Local::new(scope, &ctor_g);
    let Ok(ctor) = v8::Local::<v8::Function>::try_from(ctor_local) else {
        return NapiStatus::FunctionExpected;
    };

    let mut args_vec: Vec<v8::Local<v8::Value>> = Vec::with_capacity(argc);
    if argc > 0 {
        if argv.is_null() {
            return NapiStatus::InvalidArg;
        }
        for i in 0..argc {
            let token = unsafe { *argv.add(i) };
            let Some(g) = env.resolve(token) else {
                return NapiStatus::InvalidArg;
            };
            args_vec.push(v8::Local::new(scope, &g));
        }
    }

    v8::tc_scope!(let try_catch, scope);
    let returned = ctor.new_instance(try_catch, &args_vec);
    if let Some(exc) = try_catch.exception() {
        *env.pending_exception.borrow_mut() = Some(Global::new(try_catch, exc));
        try_catch.rethrow();
        return NapiStatus::PendingException;
    }
    let Some(obj) = returned else {
        return NapiStatus::GenericFailure;
    };
    let v: v8::Local<v8::Value> = obj.into();
    unsafe { write_value_out(env, result, v, try_catch) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_create_error(
    env: napi_env,
    code: napi_value,
    msg: napi_value,
    result: *mut napi_value,
) -> NapiStatus {
    unsafe { create_error(env, code, msg, result, ErrKind::Error) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_create_type_error(
    env: napi_env,
    code: napi_value,
    msg: napi_value,
    result: *mut napi_value,
) -> NapiStatus {
    unsafe { create_error(env, code, msg, result, ErrKind::Type) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_create_range_error(
    env: napi_env,
    code: napi_value,
    msg: napi_value,
    result: *mut napi_value,
) -> NapiStatus {
    unsafe { create_error(env, code, msg, result, ErrKind::Range) }
}

unsafe fn create_error(
    env: napi_env,
    code: napi_value,
    msg: napi_value,
    result: *mut napi_value,
    kind: ErrKind,
) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    let Some(msg_g) = env.resolve(msg) else {
        return NapiStatus::InvalidArg;
    };
    let scope = unsafe { scope_from_env(env) };
    let msg_local = v8::Local::new(scope, &msg_g);
    let Ok(msg_str) = v8::Local::<v8::String>::try_from(msg_local) else {
        return NapiStatus::StringExpected;
    };
    let err: v8::Local<v8::Value> = match kind {
        ErrKind::Error => v8::Exception::error(scope, msg_str),
        ErrKind::Type => v8::Exception::type_error(scope, msg_str),
        ErrKind::Range => v8::Exception::range_error(scope, msg_str),
    };
    if !code.0.is_null()
        && let Some(code_g) = env.resolve(code)
        && let Ok(err_obj) = v8::Local::<v8::Object>::try_from(err)
        && let Some(key) = v8::String::new(scope, "code")
    {
        let code_local = v8::Local::new(scope, &code_g);
        let key_v: v8::Local<v8::Value> = key.into();
        let _ = err_obj.set(scope, key_v, code_local);
    }
    unsafe { write_value_out(env, result, err, scope) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_get_and_clear_last_exception(
    env: napi_env,
    result: *mut napi_value,
) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    if result.is_null() {
        return NapiStatus::InvalidArg;
    }
    let pending = env.pending_exception.borrow_mut().take();
    let scope = unsafe { scope_from_env(env) };
    match pending {
        Some(g) => {
            let local = v8::Local::new(scope, &g);
            unsafe { write_value_out(env, result, local, scope) }
        }
        None => {
            let v: v8::Local<v8::Value> = v8::undefined(scope).into();
            unsafe { write_value_out(env, result, v, scope) }
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_strict_equals(
    env: napi_env,
    lhs: napi_value,
    rhs: napi_value,
    result: *mut bool,
) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    let Some(l_g) = env.resolve(lhs) else {
        return NapiStatus::InvalidArg;
    };
    let Some(r_g) = env.resolve(rhs) else {
        return NapiStatus::InvalidArg;
    };
    if result.is_null() {
        return NapiStatus::InvalidArg;
    }
    let scope = unsafe { scope_from_env(env) };
    let l = v8::Local::new(scope, &l_g);
    let r = v8::Local::new(scope, &r_g);
    unsafe { ptr::write(result, l.strict_equals(r)) };
    NapiStatus::Ok
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_coerce_to_string(
    env: napi_env,
    value: napi_value,
    result: *mut napi_value,
) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    let Some(g) = env.resolve(value) else {
        return NapiStatus::InvalidArg;
    };
    let scope = unsafe { scope_from_env(env) };
    let local = v8::Local::new(scope, &g);
    let Some(s) = local.to_string(scope) else {
        return NapiStatus::GenericFailure;
    };
    let v: v8::Local<v8::Value> = s.into();
    unsafe { write_value_out(env, result, v, scope) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_coerce_to_number(
    env: napi_env,
    value: napi_value,
    result: *mut napi_value,
) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    let Some(g) = env.resolve(value) else {
        return NapiStatus::InvalidArg;
    };
    let scope = unsafe { scope_from_env(env) };
    let local = v8::Local::new(scope, &g);
    let Some(n) = local.to_number(scope) else {
        return NapiStatus::GenericFailure;
    };
    let v: v8::Local<v8::Value> = n.into();
    unsafe { write_value_out(env, result, v, scope) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_coerce_to_bool(
    env: napi_env,
    value: napi_value,
    result: *mut napi_value,
) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    let Some(g) = env.resolve(value) else {
        return NapiStatus::InvalidArg;
    };
    let scope = unsafe { scope_from_env(env) };
    let local = v8::Local::new(scope, &g);
    let b = local.to_boolean(scope);
    let v: v8::Local<v8::Value> = b.into();
    unsafe { write_value_out(env, result, v, scope) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_coerce_to_object(
    env: napi_env,
    value: napi_value,
    result: *mut napi_value,
) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    let Some(g) = env.resolve(value) else {
        return NapiStatus::InvalidArg;
    };
    let scope = unsafe { scope_from_env(env) };
    let local = v8::Local::new(scope, &g);
    let Some(o) = local.to_object(scope) else {
        return NapiStatus::GenericFailure;
    };
    let v: v8::Local<v8::Value> = o.into();
    unsafe { write_value_out(env, result, v, scope) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_get_value_int32(
    env: napi_env,
    value: napi_value,
    result: *mut i32,
) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    let Some(g) = env.resolve(value) else {
        return NapiStatus::InvalidArg;
    };
    let scope = unsafe { scope_from_env(env) };
    let local = v8::Local::new(scope, &g);
    let Some(n) = local.int32_value(scope) else {
        return NapiStatus::NumberExpected;
    };
    if result.is_null() {
        return NapiStatus::InvalidArg;
    }
    unsafe { ptr::write(result, n) };
    NapiStatus::Ok
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_get_value_uint32(
    env: napi_env,
    value: napi_value,
    result: *mut u32,
) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    let Some(g) = env.resolve(value) else {
        return NapiStatus::InvalidArg;
    };
    let scope = unsafe { scope_from_env(env) };
    let local = v8::Local::new(scope, &g);
    let Some(n) = local.uint32_value(scope) else {
        return NapiStatus::NumberExpected;
    };
    if result.is_null() {
        return NapiStatus::InvalidArg;
    }
    unsafe { ptr::write(result, n) };
    NapiStatus::Ok
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_get_value_int64(
    env: napi_env,
    value: napi_value,
    result: *mut i64,
) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    let Some(g) = env.resolve(value) else {
        return NapiStatus::InvalidArg;
    };
    let scope = unsafe { scope_from_env(env) };
    let local = v8::Local::new(scope, &g);
    let Some(n) = local.integer_value(scope) else {
        return NapiStatus::NumberExpected;
    };
    if result.is_null() {
        return NapiStatus::InvalidArg;
    }
    unsafe { ptr::write(result, n) };
    NapiStatus::Ok
}

#[repr(C)]
struct ExternalDataCtx {
    finalize: Option<NapiFinalize>,
    hint: *mut c_void,
}

unsafe extern "C" fn external_data_deleter(
    data: *mut c_void,
    _byte_length: usize,
    deleter_data: *mut c_void,
) {
    if deleter_data.is_null() {
        return;
    }
    let ctx = unsafe { Box::from_raw(deleter_data.cast::<ExternalDataCtx>()) };
    if let Some(fin) = ctx.finalize {
        unsafe { fin(std::ptr::null_mut(), data, ctx.hint) };
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_create_arraybuffer(
    env: napi_env,
    byte_length: usize,
    data: *mut *mut c_void,
    result: *mut napi_value,
) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    let scope = unsafe { scope_from_env(env) };
    let ab = v8::ArrayBuffer::new(scope, byte_length);
    if !data.is_null() {
        let ptr = ab.data().map_or(ptr::null_mut(), |p| p.as_ptr());
        unsafe { ptr::write(data, ptr) };
    }
    let v: v8::Local<v8::Value> = ab.into();
    unsafe { write_value_out(env, result, v, scope) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_create_external_arraybuffer(
    env: napi_env,
    external_data: *mut c_void,
    byte_length: usize,
    finalize_cb: Option<NapiFinalize>,
    finalize_hint: *mut c_void,
    result: *mut napi_value,
) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    if external_data.is_null() && byte_length != 0 {
        return NapiStatus::InvalidArg;
    }
    let scope = unsafe { scope_from_env(env) };
    let ctx = Box::new(ExternalDataCtx {
        finalize: finalize_cb,
        hint: finalize_hint,
    });
    let ctx_ptr = Box::into_raw(ctx).cast::<c_void>();
    let backing = unsafe {
        v8::ArrayBuffer::new_backing_store_from_ptr(
            external_data,
            byte_length,
            external_data_deleter,
            ctx_ptr,
        )
    };
    let shared = backing.make_shared();
    let ab = v8::ArrayBuffer::with_backing_store(scope, &shared);
    let v: v8::Local<v8::Value> = ab.into();
    unsafe { write_value_out(env, result, v, scope) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_get_arraybuffer_info(
    env: napi_env,
    arraybuffer: napi_value,
    data: *mut *mut c_void,
    byte_length: *mut usize,
) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    let Some(g) = env.resolve(arraybuffer) else {
        return NapiStatus::InvalidArg;
    };
    let scope = unsafe { scope_from_env(env) };
    let local = v8::Local::new(scope, &g);
    let Ok(ab) = v8::Local::<v8::ArrayBuffer>::try_from(local) else {
        return NapiStatus::ArraybufferExpected;
    };
    if !data.is_null() {
        let ptr = ab.data().map_or(ptr::null_mut(), |p| p.as_ptr());
        unsafe { ptr::write(data, ptr) };
    }
    if !byte_length.is_null() {
        unsafe { ptr::write(byte_length, ab.byte_length()) };
    }
    NapiStatus::Ok
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_is_arraybuffer(
    env: napi_env,
    value: napi_value,
    result: *mut bool,
) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    let Some(g) = env.resolve(value) else {
        return NapiStatus::InvalidArg;
    };
    if result.is_null() {
        return NapiStatus::InvalidArg;
    }
    let scope = unsafe { scope_from_env(env) };
    let local = v8::Local::new(scope, &g);
    unsafe { ptr::write(result, local.is_array_buffer()) };
    NapiStatus::Ok
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_detach_arraybuffer(
    env: napi_env,
    arraybuffer: napi_value,
) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    let Some(g) = env.resolve(arraybuffer) else {
        return NapiStatus::InvalidArg;
    };
    let scope = unsafe { scope_from_env(env) };
    let local = v8::Local::new(scope, &g);
    let Ok(ab) = v8::Local::<v8::ArrayBuffer>::try_from(local) else {
        return NapiStatus::ArraybufferExpected;
    };
    if !ab.is_detachable() {
        return NapiStatus::DetachableArraybufferExpected;
    }
    let _ = ab.detach(None);
    NapiStatus::Ok
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_is_detached_arraybuffer(
    env: napi_env,
    arraybuffer: napi_value,
    result: *mut bool,
) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    let Some(g) = env.resolve(arraybuffer) else {
        return NapiStatus::InvalidArg;
    };
    if result.is_null() {
        return NapiStatus::InvalidArg;
    }
    let scope = unsafe { scope_from_env(env) };
    let local = v8::Local::new(scope, &g);
    let detached = v8::Local::<v8::ArrayBuffer>::try_from(local)
        .map(|ab| ab.was_detached())
        .unwrap_or(false);
    unsafe { ptr::write(result, detached) };
    NapiStatus::Ok
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_create_typedarray(
    env: napi_env,
    type_: NapiTypedArrayType,
    length: usize,
    arraybuffer: napi_value,
    byte_offset: usize,
    result: *mut napi_value,
) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    let Some(g) = env.resolve(arraybuffer) else {
        return NapiStatus::InvalidArg;
    };
    let scope = unsafe { scope_from_env(env) };
    let local = v8::Local::new(scope, &g);
    let Ok(ab) = v8::Local::<v8::ArrayBuffer>::try_from(local) else {
        return NapiStatus::ArraybufferExpected;
    };
    let Some(ta) = (match type_ {
        NapiTypedArrayType::Int8 => v8::Int8Array::new(scope, ab, byte_offset, length)
            .map(v8::Local::<v8::Value>::from),
        NapiTypedArrayType::Uint8 => v8::Uint8Array::new(scope, ab, byte_offset, length)
            .map(v8::Local::<v8::Value>::from),
        NapiTypedArrayType::Uint8Clamped => {
            v8::Uint8ClampedArray::new(scope, ab, byte_offset, length)
                .map(v8::Local::<v8::Value>::from)
        }
        NapiTypedArrayType::Int16 => v8::Int16Array::new(scope, ab, byte_offset, length)
            .map(v8::Local::<v8::Value>::from),
        NapiTypedArrayType::Uint16 => v8::Uint16Array::new(scope, ab, byte_offset, length)
            .map(v8::Local::<v8::Value>::from),
        NapiTypedArrayType::Int32 => v8::Int32Array::new(scope, ab, byte_offset, length)
            .map(v8::Local::<v8::Value>::from),
        NapiTypedArrayType::Uint32 => v8::Uint32Array::new(scope, ab, byte_offset, length)
            .map(v8::Local::<v8::Value>::from),
        NapiTypedArrayType::Float32 => v8::Float32Array::new(scope, ab, byte_offset, length)
            .map(v8::Local::<v8::Value>::from),
        NapiTypedArrayType::Float64 => v8::Float64Array::new(scope, ab, byte_offset, length)
            .map(v8::Local::<v8::Value>::from),
        NapiTypedArrayType::BigInt64 => v8::BigInt64Array::new(scope, ab, byte_offset, length)
            .map(v8::Local::<v8::Value>::from),
        NapiTypedArrayType::BigUint64 => v8::BigUint64Array::new(scope, ab, byte_offset, length)
            .map(v8::Local::<v8::Value>::from),
    }) else {
        return NapiStatus::GenericFailure;
    };
    unsafe { write_value_out(env, result, ta, scope) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_get_typedarray_info(
    env: napi_env,
    typedarray: napi_value,
    type_out: *mut NapiTypedArrayType,
    length_out: *mut usize,
    data_out: *mut *mut c_void,
    arraybuffer_out: *mut napi_value,
    byte_offset_out: *mut usize,
) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    let Some(g) = env.resolve(typedarray) else {
        return NapiStatus::InvalidArg;
    };
    let scope = unsafe { scope_from_env(env) };
    let local = v8::Local::new(scope, &g);
    let Ok(view) = v8::Local::<v8::ArrayBufferView>::try_from(local) else {
        return NapiStatus::InvalidArg;
    };
    if !local.is_typed_array() {
        return NapiStatus::InvalidArg;
    }
    if !type_out.is_null() {
        let kind = if local.is_int8_array() {
            NapiTypedArrayType::Int8
        } else if local.is_uint8_clamped_array() {
            NapiTypedArrayType::Uint8Clamped
        } else if local.is_uint8_array() {
            NapiTypedArrayType::Uint8
        } else if local.is_int16_array() {
            NapiTypedArrayType::Int16
        } else if local.is_uint16_array() {
            NapiTypedArrayType::Uint16
        } else if local.is_int32_array() {
            NapiTypedArrayType::Int32
        } else if local.is_uint32_array() {
            NapiTypedArrayType::Uint32
        } else if local.is_float32_array() {
            NapiTypedArrayType::Float32
        } else if local.is_float64_array() {
            NapiTypedArrayType::Float64
        } else {
            return NapiStatus::GenericFailure;
        };
        unsafe { ptr::write(type_out, kind) };
    }
    let Ok(ta) = v8::Local::<v8::TypedArray>::try_from(local) else {
        return NapiStatus::InvalidArg;
    };
    if !length_out.is_null() {
        unsafe { ptr::write(length_out, ta.length()) };
    }
    let byte_offset = view.byte_offset();
    if !byte_offset_out.is_null() {
        unsafe { ptr::write(byte_offset_out, byte_offset) };
    }
    let ab = view.buffer(scope);
    if !data_out.is_null() {
        let base = ab
            .as_ref()
            .and_then(|b| b.data())
            .map_or(ptr::null_mut(), |p| p.as_ptr());
        let with_offset = if base.is_null() {
            ptr::null_mut()
        } else {
            unsafe { base.cast::<u8>().add(byte_offset).cast::<c_void>() }
        };
        unsafe { ptr::write(data_out, with_offset) };
    }
    if !arraybuffer_out.is_null() {
        match ab {
            Some(b) => {
                let v: v8::Local<v8::Value> = b.into();
                let g = Global::new(scope, v);
                unsafe { ptr::write(arraybuffer_out, env.intern(g)) };
            }
            None => unsafe { ptr::write(arraybuffer_out, napi_value(ptr::null_mut())) },
        }
    }
    NapiStatus::Ok
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_is_typedarray(
    env: napi_env,
    value: napi_value,
    result: *mut bool,
) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    let Some(g) = env.resolve(value) else {
        return NapiStatus::InvalidArg;
    };
    if result.is_null() {
        return NapiStatus::InvalidArg;
    }
    let scope = unsafe { scope_from_env(env) };
    let local = v8::Local::new(scope, &g);
    unsafe { ptr::write(result, local.is_typed_array()) };
    NapiStatus::Ok
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_create_dataview(
    env: napi_env,
    byte_length: usize,
    arraybuffer: napi_value,
    byte_offset: usize,
    result: *mut napi_value,
) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    let Some(g) = env.resolve(arraybuffer) else {
        return NapiStatus::InvalidArg;
    };
    let scope = unsafe { scope_from_env(env) };
    let local = v8::Local::new(scope, &g);
    let Ok(ab) = v8::Local::<v8::ArrayBuffer>::try_from(local) else {
        return NapiStatus::ArraybufferExpected;
    };
    let dv = v8::DataView::new(scope, ab, byte_offset, byte_length);
    let v: v8::Local<v8::Value> = dv.into();
    unsafe { write_value_out(env, result, v, scope) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_get_dataview_info(
    env: napi_env,
    dataview: napi_value,
    byte_length_out: *mut usize,
    data_out: *mut *mut c_void,
    arraybuffer_out: *mut napi_value,
    byte_offset_out: *mut usize,
) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    let Some(g) = env.resolve(dataview) else {
        return NapiStatus::InvalidArg;
    };
    let scope = unsafe { scope_from_env(env) };
    let local = v8::Local::new(scope, &g);
    if !local.is_data_view() {
        return NapiStatus::InvalidArg;
    }
    let Ok(view) = v8::Local::<v8::ArrayBufferView>::try_from(local) else {
        return NapiStatus::InvalidArg;
    };
    let byte_offset = view.byte_offset();
    let byte_length = view.byte_length();
    if !byte_length_out.is_null() {
        unsafe { ptr::write(byte_length_out, byte_length) };
    }
    if !byte_offset_out.is_null() {
        unsafe { ptr::write(byte_offset_out, byte_offset) };
    }
    let ab = view.buffer(scope);
    if !data_out.is_null() {
        let base = ab
            .as_ref()
            .and_then(|b| b.data())
            .map_or(ptr::null_mut(), |p| p.as_ptr());
        let with_offset = if base.is_null() {
            ptr::null_mut()
        } else {
            unsafe { base.cast::<u8>().add(byte_offset).cast::<c_void>() }
        };
        unsafe { ptr::write(data_out, with_offset) };
    }
    if !arraybuffer_out.is_null() {
        match ab {
            Some(b) => {
                let v: v8::Local<v8::Value> = b.into();
                let g = Global::new(scope, v);
                unsafe { ptr::write(arraybuffer_out, env.intern(g)) };
            }
            None => unsafe { ptr::write(arraybuffer_out, napi_value(ptr::null_mut())) },
        }
    }
    NapiStatus::Ok
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_is_dataview(
    env: napi_env,
    value: napi_value,
    result: *mut bool,
) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    let Some(g) = env.resolve(value) else {
        return NapiStatus::InvalidArg;
    };
    if result.is_null() {
        return NapiStatus::InvalidArg;
    }
    let scope = unsafe { scope_from_env(env) };
    let local = v8::Local::new(scope, &g);
    unsafe { ptr::write(result, local.is_data_view()) };
    NapiStatus::Ok
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_create_buffer(
    env: napi_env,
    length: usize,
    data: *mut *mut c_void,
    result: *mut napi_value,
) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    let scope = unsafe { scope_from_env(env) };
    let ab = v8::ArrayBuffer::new(scope, length);
    if !data.is_null() {
        let ptr = ab.data().map_or(ptr::null_mut(), |p| p.as_ptr());
        unsafe { ptr::write(data, ptr) };
    }
    let Some(view) = v8::Uint8Array::new(scope, ab, 0, length) else {
        return NapiStatus::GenericFailure;
    };
    let v: v8::Local<v8::Value> = view.into();
    unsafe { write_value_out(env, result, v, scope) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_create_buffer_copy(
    env: napi_env,
    length: usize,
    data: *const c_void,
    result_data: *mut *mut c_void,
    result: *mut napi_value,
) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    if length != 0 && data.is_null() {
        return NapiStatus::InvalidArg;
    }
    let scope = unsafe { scope_from_env(env) };
    let ab = v8::ArrayBuffer::new(scope, length);
    let ptr = ab.data().map_or(ptr::null_mut(), |p| p.as_ptr());
    if length != 0 && !ptr.is_null() {
        unsafe { ptr::copy_nonoverlapping(data.cast::<u8>(), ptr.cast::<u8>(), length) };
    }
    if !result_data.is_null() {
        unsafe { ptr::write(result_data, ptr) };
    }
    let Some(view) = v8::Uint8Array::new(scope, ab, 0, length) else {
        return NapiStatus::GenericFailure;
    };
    let v: v8::Local<v8::Value> = view.into();
    unsafe { write_value_out(env, result, v, scope) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_create_external_buffer(
    env: napi_env,
    length: usize,
    data: *mut c_void,
    finalize_cb: Option<NapiFinalize>,
    finalize_hint: *mut c_void,
    result: *mut napi_value,
) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    if length != 0 && data.is_null() {
        return NapiStatus::InvalidArg;
    }
    let scope = unsafe { scope_from_env(env) };
    let ctx = Box::new(ExternalDataCtx {
        finalize: finalize_cb,
        hint: finalize_hint,
    });
    let ctx_ptr = Box::into_raw(ctx).cast::<c_void>();
    let backing = unsafe {
        v8::ArrayBuffer::new_backing_store_from_ptr(data, length, external_data_deleter, ctx_ptr)
    };
    let shared = backing.make_shared();
    let ab = v8::ArrayBuffer::with_backing_store(scope, &shared);
    let Some(view) = v8::Uint8Array::new(scope, ab, 0, length) else {
        return NapiStatus::GenericFailure;
    };
    let v: v8::Local<v8::Value> = view.into();
    unsafe { write_value_out(env, result, v, scope) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_get_buffer_info(
    env: napi_env,
    value: napi_value,
    data_out: *mut *mut c_void,
    length_out: *mut usize,
) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    let Some(g) = env.resolve(value) else {
        return NapiStatus::InvalidArg;
    };
    let scope = unsafe { scope_from_env(env) };
    let local = v8::Local::new(scope, &g);
    let Ok(view) = v8::Local::<v8::ArrayBufferView>::try_from(local) else {
        return NapiStatus::InvalidArg;
    };
    if !length_out.is_null() {
        unsafe { ptr::write(length_out, view.byte_length()) };
    }
    if !data_out.is_null() {
        let byte_offset = view.byte_offset();
        let base = view
            .buffer(scope)
            .as_ref()
            .and_then(|b| b.data())
            .map_or(ptr::null_mut(), |p| p.as_ptr());
        let with_offset = if base.is_null() {
            ptr::null_mut()
        } else {
            unsafe { base.cast::<u8>().add(byte_offset).cast::<c_void>() }
        };
        unsafe { ptr::write(data_out, with_offset) };
    }
    NapiStatus::Ok
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_is_buffer(
    env: napi_env,
    value: napi_value,
    result: *mut bool,
) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    let Some(g) = env.resolve(value) else {
        return NapiStatus::InvalidArg;
    };
    if result.is_null() {
        return NapiStatus::InvalidArg;
    }
    let scope = unsafe { scope_from_env(env) };
    let local = v8::Local::new(scope, &g);
    unsafe { ptr::write(result, local.is_uint8_array()) };
    NapiStatus::Ok
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_create_external(
    env: napi_env,
    data: *mut c_void,
    _finalize_cb: Option<NapiFinalize>,
    _finalize_hint: *mut c_void,
    result: *mut napi_value,
) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    let scope = unsafe { scope_from_env(env) };
    let ext = v8::External::new(scope, data);
    let v: v8::Local<v8::Value> = ext.into();
    unsafe { write_value_out(env, result, v, scope) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_get_value_external(
    env: napi_env,
    value: napi_value,
    result: *mut *mut c_void,
) -> NapiStatus {
    let Some(env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    let Some(g) = env.resolve(value) else {
        return NapiStatus::InvalidArg;
    };
    if result.is_null() {
        return NapiStatus::InvalidArg;
    }
    let scope = unsafe { scope_from_env(env) };
    let local = v8::Local::new(scope, &g);
    let Ok(ext) = v8::Local::<v8::External>::try_from(local) else {
        return NapiStatus::InvalidArg;
    };
    unsafe { ptr::write(result, ext.value()) };
    NapiStatus::Ok
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_create_reference(
    env: napi_env,
    value: napi_value,
    initial_refcount: u32,
    result: *mut napi_ref,
) -> NapiStatus {
    let Some(env_ref_) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    if result.is_null() {
        return NapiStatus::InvalidArg;
    }
    let Some(global) = env_ref_.resolve(value) else {
        return NapiStatus::InvalidArg;
    };
    let inner = RefInner::boxed(global, initial_refcount);
    unsafe { ptr::write(result, napi_ref(inner.cast::<c_void>())) };
    NapiStatus::Ok
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_delete_reference(
    env: napi_env,
    reference: napi_ref,
) -> NapiStatus {
    if (unsafe { env_ref(env) }).is_none() {
        return NapiStatus::InvalidArg;
    }
    if reference.0.is_null() {
        return NapiStatus::InvalidArg;
    }
    unsafe { RefInner::drop_raw(reference.0.cast::<RefInner>()) };
    NapiStatus::Ok
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_reference_ref(
    env: napi_env,
    reference: napi_ref,
    result: *mut u32,
) -> NapiStatus {
    if (unsafe { env_ref(env) }).is_none() {
        return NapiStatus::InvalidArg;
    }
    if reference.0.is_null() {
        return NapiStatus::InvalidArg;
    }
    let inner = unsafe { &mut *reference.0.cast::<RefInner>() };
    inner.count = inner.count.saturating_add(1);
    if !result.is_null() {
        unsafe { ptr::write(result, inner.count) };
    }
    NapiStatus::Ok
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_reference_unref(
    env: napi_env,
    reference: napi_ref,
    result: *mut u32,
) -> NapiStatus {
    if (unsafe { env_ref(env) }).is_none() {
        return NapiStatus::InvalidArg;
    }
    if reference.0.is_null() {
        return NapiStatus::InvalidArg;
    }
    let inner = unsafe { &mut *reference.0.cast::<RefInner>() };
    if inner.count == 0 {
        return NapiStatus::GenericFailure;
    }
    inner.count -= 1;
    if !result.is_null() {
        unsafe { ptr::write(result, inner.count) };
    }
    NapiStatus::Ok
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_get_reference_value(
    env: napi_env,
    reference: napi_ref,
    result: *mut napi_value,
) -> NapiStatus {
    let Some(env_ref_) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    if result.is_null() {
        return NapiStatus::InvalidArg;
    }
    if reference.0.is_null() {
        return NapiStatus::InvalidArg;
    }
    let inner = unsafe { &*reference.0.cast::<RefInner>() };
    let scope = unsafe { scope_from_env(env_ref_) };
    let local = v8::Local::new(scope, &inner.global);
    let global = Global::new(scope, local);
    unsafe { ptr::write(result, env_ref_.intern(global)) };
    NapiStatus::Ok
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_create_async_work(
    env: napi_env,
    _async_resource: napi_value,
    _async_resource_name: napi_value,
    execute: Option<NapiAsyncExecute>,
    complete: Option<NapiAsyncComplete>,
    data: *mut c_void,
    result: *mut napi_async_work,
) -> NapiStatus {
    let Some(_env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    let Some(execute) = execute else {
        return NapiStatus::InvalidArg;
    };
    if result.is_null() {
        return NapiStatus::InvalidArg;
    }
    let inner = AsyncWorkInner::boxed(execute, complete, data);
    unsafe { ptr::write(result, napi_async_work(inner.cast::<c_void>())) };
    NapiStatus::Ok
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_delete_async_work(
    env: napi_env,
    work: napi_async_work,
) -> NapiStatus {
    let Some(_env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    if work.0.is_null() {
        return NapiStatus::InvalidArg;
    }
    unsafe { AsyncWorkInner::drop_raw(work.0.cast::<AsyncWorkInner>()) };
    NapiStatus::Ok
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_cancel_async_work(
    env: napi_env,
    work: napi_async_work,
) -> NapiStatus {
    let Some(_env) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    if work.0.is_null() {
        return NapiStatus::InvalidArg;
    }
    unsafe { crate::napi::async_work::cancel(work.0.cast::<AsyncWorkInner>()) };
    NapiStatus::Ok
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_queue_async_work(
    env: napi_env,
    work: napi_async_work,
) -> NapiStatus {
    let Some(env_ref_) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    if work.0.is_null() {
        return NapiStatus::InvalidArg;
    }
    let inner_ptr = work.0.cast::<AsyncWorkInner>();

    let scope = unsafe { scope_from_env(env_ref_) };
    let isolate: &v8::Isolate = scope;
    let tx = crate::engine::v8_engine::napi_work_sender(isolate);

    let inner_addr = inner_ptr as usize;
    {
        let inner = unsafe { &*inner_ptr };
        if inner.queued.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return NapiStatus::GenericFailure;
        }
    }
    let Ok(rt) = tokio::runtime::Handle::try_current() else {
        return NapiStatus::GenericFailure;
    };
    rt.spawn_blocking(move || {
        let inner_ptr = inner_addr as *mut AsyncWorkInner;
        let inner_ref = unsafe { &*inner_ptr };
        let cancelled = inner_ref.cancelled.load(std::sync::atomic::Ordering::SeqCst);
        if !cancelled {
            unsafe { (inner_ref.execute)(std::ptr::null_mut(), inner_ref.data.0) };
        }
        let status = if cancelled {
            NapiStatus::Cancelled
        } else {
            NapiStatus::Ok
        };
        let work_box: NapiWorkItem = Box::new(move |scope: &mut v8::PinScope<'_, '_>| {
            let inner_ptr = inner_addr as *mut AsyncWorkInner;
            let inner = unsafe { &*inner_ptr };
            if let Some(complete) = inner.complete {
                let scope_ptr: *mut c_void =
                    std::ptr::from_mut::<v8::PinScope<'_, '_>>(scope).cast();
                let context = scope.get_current_context();
                let context_box = Box::new(v8::Global::new(scope, context));
                let context_ptr = Box::into_raw(context_box).cast::<c_void>();
                let mut cb_env = NapiEnv::new(scope_ptr, context_ptr);
                let env_ptr: napi_env = std::ptr::from_mut(&mut cb_env);
                unsafe { complete(env_ptr, status, inner.data.0) };
                drop(unsafe {
                    Box::from_raw(context_ptr.cast::<v8::Global<v8::Context>>())
                });
            }
        });
        let _ = tx.send(work_box);
    });
    NapiStatus::Ok
}

// ──────────────────────────────────────────────────────────────────────
// threadsafe-functions
// ──────────────────────────────────────────────────────────────────────

unsafe fn tsfn_inner<'a>(tsfn: napi_threadsafe_function) -> Option<&'a TsfnInner> {
    if tsfn.0.is_null() {
        return None;
    }
    Some(unsafe { &*tsfn.0.cast::<TsfnInner>() })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_create_threadsafe_function(
    env: napi_env,
    func: napi_value,
    _async_resource: napi_value,
    _async_resource_name: napi_value,
    _max_queue_size: usize,
    initial_thread_count: usize,
    thread_finalize_data: *mut c_void,
    thread_finalize_cb: Option<NapiFinalize>,
    context: *mut c_void,
    call_js_cb: Option<NapiThreadsafeFunctionCallJs>,
    result: *mut napi_threadsafe_function,
) -> NapiStatus {
    let Some(env_ref_) = (unsafe { env_ref(env) }) else {
        return NapiStatus::InvalidArg;
    };
    if result.is_null() || initial_thread_count == 0 {
        return NapiStatus::InvalidArg;
    }
    let js_callback = if func.0.is_null() {
        None
    } else {
        env_ref_.resolve(func)
    };
    if js_callback.is_none() && call_js_cb.is_none() {
        return NapiStatus::InvalidArg;
    }
    let scope = unsafe { scope_from_env(env_ref_) };
    let isolate: &v8::Isolate = scope;
    let work_tx = napi_work_sender(isolate);

    let inner = TsfnInner {
        js_callback,
        call_js: call_js_cb,
        context,
        finalize: thread_finalize_cb,
        finalize_data: thread_finalize_data,
        thread_count: std::sync::atomic::AtomicUsize::new(initial_thread_count),
        aborted: std::sync::atomic::AtomicBool::new(false),
        kept_alive: std::sync::atomic::AtomicBool::new(true),
        queued: std::sync::atomic::AtomicIsize::new(0),
        work_tx,
    };
    let raw = TsfnInner::boxed(inner);
    unsafe { ptr::write(result, napi_threadsafe_function(raw.cast::<c_void>())) };
    NapiStatus::Ok
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_get_threadsafe_function_context(
    func: napi_threadsafe_function,
    result: *mut *mut c_void,
) -> NapiStatus {
    let Some(inner) = (unsafe { tsfn_inner(func) }) else {
        return NapiStatus::InvalidArg;
    };
    if result.is_null() {
        return NapiStatus::InvalidArg;
    }
    unsafe { ptr::write(result, inner.context) };
    NapiStatus::Ok
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_acquire_threadsafe_function(
    func: napi_threadsafe_function,
) -> NapiStatus {
    let Some(inner) = (unsafe { tsfn_inner(func) }) else {
        return NapiStatus::InvalidArg;
    };
    if inner.aborted.load(std::sync::atomic::Ordering::SeqCst) {
        return NapiStatus::Closing;
    }
    inner
        .thread_count
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    NapiStatus::Ok
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_release_threadsafe_function(
    func: napi_threadsafe_function,
    mode: NapiThreadsafeFunctionReleaseMode,
) -> NapiStatus {
    let Some(inner) = (unsafe { tsfn_inner(func) }) else {
        return NapiStatus::InvalidArg;
    };
    if mode == NapiThreadsafeFunctionReleaseMode::Abort {
        inner.aborted.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    let prev = inner
        .thread_count
        .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    if prev == 0 {
        inner.thread_count.store(0, std::sync::atomic::Ordering::SeqCst);
        return NapiStatus::InvalidArg;
    }
    if prev == 1 {
        // last thread released — schedule finalize on JS thread.
        let tsfn_addr = func.0 as usize;
        let _ = inner.work_tx.send(Box::new(move |scope: &mut v8::PinScope<'_, '_>| {
            let raw = tsfn_addr as *mut TsfnInner;
            let owned = unsafe { &*raw };
            if let Some(finalize) = owned.finalize {
                let scope_ptr: *mut c_void =
                    std::ptr::from_mut::<v8::PinScope<'_, '_>>(scope).cast();
                let context = scope.get_current_context();
                let context_box = Box::new(v8::Global::new(scope, context));
                let context_ptr = Box::into_raw(context_box).cast::<c_void>();
                let mut cb_env = NapiEnv::new(scope_ptr, context_ptr);
                let env_ptr: napi_env = std::ptr::from_mut(&mut cb_env);
                unsafe { finalize(env_ptr, owned.finalize_data, owned.context) };
                drop(unsafe { Box::from_raw(context_ptr.cast::<v8::Global<v8::Context>>()) });
            }
            unsafe { TsfnInner::drop_raw(raw) };
        }));
    }
    NapiStatus::Ok
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_call_threadsafe_function(
    func: napi_threadsafe_function,
    data: *mut c_void,
    _is_blocking: NapiThreadsafeFunctionCallMode,
) -> NapiStatus {
    let Some(inner) = (unsafe { tsfn_inner(func) }) else {
        return NapiStatus::InvalidArg;
    };
    if inner.aborted.load(std::sync::atomic::Ordering::SeqCst) {
        return NapiStatus::Closing;
    }
    inner
        .queued
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

    let tsfn_addr = func.0 as usize;
    let data_addr = data as usize;
    let work_box: NapiWorkItem = Box::new(move |scope: &mut v8::PinScope<'_, '_>| {
        let raw = tsfn_addr as *mut TsfnInner;
        let owned = unsafe { &*raw };
        owned
            .queued
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        if owned.aborted.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let data_ptr = data_addr as *mut c_void;
        let scope_ptr: *mut c_void =
            std::ptr::from_mut::<v8::PinScope<'_, '_>>(scope).cast();
        let context = scope.get_current_context();
        let context_box = Box::new(v8::Global::new(scope, context));
        let context_ptr = Box::into_raw(context_box).cast::<c_void>();
        let mut cb_env = NapiEnv::new(scope_ptr, context_ptr);
        let env_ptr: napi_env = std::ptr::from_mut(&mut cb_env);

        let js_callback_token = if let Some(global) = owned.js_callback.as_ref() {
            cb_env.intern(global.clone())
        } else {
            napi_value(std::ptr::null_mut())
        };

        if let Some(call_js) = owned.call_js {
            unsafe { call_js(env_ptr, js_callback_token, owned.context, data_ptr) };
        } else if let Some(global) = owned.js_callback.as_ref() {
            // Default: invoke func.call(undefined) with no args.
            let local = v8::Local::new(scope, global);
            if let Ok(func_local) = v8::Local::<v8::Function>::try_from(local) {
                let recv = v8::undefined(scope).into();
                let _ = func_local.call(scope, recv, &[]);
            }
        }
        drop(unsafe { Box::from_raw(context_ptr.cast::<v8::Global<v8::Context>>()) });
    });
    if inner.work_tx.send(work_box).is_err() {
        return NapiStatus::Closing;
    }
    NapiStatus::Ok
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_ref_threadsafe_function(
    env: napi_env,
    func: napi_threadsafe_function,
) -> NapiStatus {
    if (unsafe { env_ref(env) }).is_none() {
        return NapiStatus::InvalidArg;
    }
    let Some(inner) = (unsafe { tsfn_inner(func) }) else {
        return NapiStatus::InvalidArg;
    };
    inner
        .kept_alive
        .store(true, std::sync::atomic::Ordering::SeqCst);
    NapiStatus::Ok
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_unref_threadsafe_function(
    env: napi_env,
    func: napi_threadsafe_function,
) -> NapiStatus {
    if (unsafe { env_ref(env) }).is_none() {
        return NapiStatus::InvalidArg;
    }
    let Some(inner) = (unsafe { tsfn_inner(func) }) else {
        return NapiStatus::InvalidArg;
    };
    inner
        .kept_alive
        .store(false, std::sync::atomic::Ordering::SeqCst);
    NapiStatus::Ok
}

#[repr(transparent)]
struct ForceExport(*const c_void);
unsafe impl Sync for ForceExport {}

#[used]
static NAPI_EXPORTS: [ForceExport; 78] = [
    ForceExport(napi_get_undefined as *const c_void),
    ForceExport(napi_get_null as *const c_void),
    ForceExport(napi_get_boolean as *const c_void),
    ForceExport(napi_create_double as *const c_void),
    ForceExport(napi_create_int32 as *const c_void),
    ForceExport(napi_create_uint32 as *const c_void),
    ForceExport(napi_create_int64 as *const c_void),
    ForceExport(napi_create_string_utf8 as *const c_void),
    ForceExport(napi_get_value_string_utf8 as *const c_void),
    ForceExport(napi_typeof as *const c_void),
    ForceExport(napi_get_value_double as *const c_void),
    ForceExport(napi_get_value_bool as *const c_void),
    ForceExport(napi_get_value_int32 as *const c_void),
    ForceExport(napi_get_value_uint32 as *const c_void),
    ForceExport(napi_get_value_int64 as *const c_void),
    ForceExport(napi_create_object as *const c_void),
    ForceExport(napi_create_array as *const c_void),
    ForceExport(napi_set_named_property as *const c_void),
    ForceExport(napi_get_named_property as *const c_void),
    ForceExport(napi_get_global as *const c_void),
    ForceExport(napi_throw as *const c_void),
    ForceExport(napi_throw_error as *const c_void),
    ForceExport(napi_throw_type_error as *const c_void),
    ForceExport(napi_throw_range_error as *const c_void),
    ForceExport(napi_create_error as *const c_void),
    ForceExport(napi_create_type_error as *const c_void),
    ForceExport(napi_create_range_error as *const c_void),
    ForceExport(napi_is_exception_pending as *const c_void),
    ForceExport(napi_get_and_clear_last_exception as *const c_void),
    ForceExport(napi_get_version as *const c_void),
    ForceExport(napi_module_register as *const c_void),
    ForceExport(napi_set_instance_data as *const c_void),
    ForceExport(napi_get_instance_data as *const c_void),
    ForceExport(napi_create_function as *const c_void),
    ForceExport(napi_get_cb_info as *const c_void),
    ForceExport(napi_get_new_target as *const c_void),
    ForceExport(napi_call_function as *const c_void),
    ForceExport(napi_new_instance as *const c_void),
    ForceExport(napi_strict_equals as *const c_void),
    ForceExport(napi_coerce_to_string as *const c_void),
    ForceExport(napi_coerce_to_number as *const c_void),
    ForceExport(napi_coerce_to_bool as *const c_void),
    ForceExport(napi_coerce_to_object as *const c_void),
    ForceExport(napi_create_arraybuffer as *const c_void),
    ForceExport(napi_create_external_arraybuffer as *const c_void),
    ForceExport(napi_get_arraybuffer_info as *const c_void),
    ForceExport(napi_is_arraybuffer as *const c_void),
    ForceExport(napi_detach_arraybuffer as *const c_void),
    ForceExport(napi_is_detached_arraybuffer as *const c_void),
    ForceExport(napi_create_typedarray as *const c_void),
    ForceExport(napi_get_typedarray_info as *const c_void),
    ForceExport(napi_is_typedarray as *const c_void),
    ForceExport(napi_create_dataview as *const c_void),
    ForceExport(napi_get_dataview_info as *const c_void),
    ForceExport(napi_is_dataview as *const c_void),
    ForceExport(napi_create_buffer as *const c_void),
    ForceExport(napi_create_buffer_copy as *const c_void),
    ForceExport(napi_create_external_buffer as *const c_void),
    ForceExport(napi_get_buffer_info as *const c_void),
    ForceExport(napi_is_buffer as *const c_void),
    ForceExport(napi_create_external as *const c_void),
    ForceExport(napi_get_value_external as *const c_void),
    ForceExport(napi_create_async_work as *const c_void),
    ForceExport(napi_delete_async_work as *const c_void),
    ForceExport(napi_queue_async_work as *const c_void),
    ForceExport(napi_cancel_async_work as *const c_void),
    ForceExport(napi_create_reference as *const c_void),
    ForceExport(napi_delete_reference as *const c_void),
    ForceExport(napi_reference_ref as *const c_void),
    ForceExport(napi_reference_unref as *const c_void),
    ForceExport(napi_get_reference_value as *const c_void),
    ForceExport(napi_create_threadsafe_function as *const c_void),
    ForceExport(napi_get_threadsafe_function_context as *const c_void),
    ForceExport(napi_acquire_threadsafe_function as *const c_void),
    ForceExport(napi_release_threadsafe_function as *const c_void),
    ForceExport(napi_call_threadsafe_function as *const c_void),
    ForceExport(napi_ref_threadsafe_function as *const c_void),
    ForceExport(napi_unref_threadsafe_function as *const c_void),
];
