//! ABI types mirroring `js_native_api_types.h` / `node_api_types.h`
//! (Node.js N-API v9). Layouts and integer values must match Node
//! exactly — addons compare by raw integer.

#![allow(non_camel_case_types, missing_docs)]

use crate::napi::env::NapiEnv;

pub type napi_env = *mut NapiEnv;

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct napi_value(pub *mut std::ffi::c_void);

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct napi_ref(pub *mut std::ffi::c_void);

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct napi_handle_scope(pub *mut std::ffi::c_void);

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct napi_escapable_handle_scope(pub *mut std::ffi::c_void);

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct napi_callback_info(pub *mut std::ffi::c_void);

#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NapiStatus {
    Ok = 0,
    InvalidArg = 1,
    ObjectExpected = 2,
    StringExpected = 3,
    NameExpected = 4,
    FunctionExpected = 5,
    NumberExpected = 6,
    BooleanExpected = 7,
    ArrayExpected = 8,
    GenericFailure = 9,
    PendingException = 10,
    Cancelled = 11,
    EscapeCalledTwice = 12,
    HandleScopeMismatch = 13,
    CallbackScopeMismatch = 14,
    QueueFull = 15,
    Closing = 16,
    BigintExpected = 17,
    DateExpected = 18,
    ArraybufferExpected = 19,
    DetachableArraybufferExpected = 20,
    WouldDeadlock = 21,
    NoExternalBuffersAllowed = 22,
    CannotRunJs = 23,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NapiValueType {
    Undefined = 0,
    Null = 1,
    Boolean = 2,
    Number = 3,
    String = 4,
    Symbol = 5,
    Object = 6,
    Function = 7,
    External = 8,
    BigInt = 9,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct NapiPropertyAttributes(pub i32);

impl NapiPropertyAttributes {
    pub const DEFAULT: Self = Self(0);
    pub const WRITABLE: Self = Self(1 << 0);
    pub const ENUMERABLE: Self = Self(1 << 1);
    pub const CONFIGURABLE: Self = Self(1 << 2);
    pub const STATIC: Self = Self(1 << 10);
    pub const DEFAULT_METHOD: Self = Self((1 << 0) | (1 << 2));
    pub const DEFAULT_JSPROPERTY: Self = Self((1 << 0) | (1 << 1) | (1 << 2));
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct NapiModule {
    pub nm_version: i32,
    pub nm_flags: u32,
    pub nm_filename: *const std::ffi::c_char,
    pub nm_register_func: Option<NapiAddonRegisterFn>,
    pub nm_modname: *const std::ffi::c_char,
    pub nm_priv: *mut std::ffi::c_void,
    pub reserved: [*mut std::ffi::c_void; 4],
}

pub type NapiAddonRegisterFn =
    unsafe extern "C" fn(env: napi_env, exports: napi_value) -> napi_value;

pub type NapiCallback = unsafe extern "C" fn(env: napi_env, info: napi_callback_info) -> napi_value;

pub type NapiAsyncExecute =
    unsafe extern "C" fn(env: napi_env, data: *mut std::ffi::c_void);

pub type NapiAsyncComplete =
    unsafe extern "C" fn(env: napi_env, status: NapiStatus, data: *mut std::ffi::c_void);

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct napi_async_work(pub *mut std::ffi::c_void);

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct napi_threadsafe_function(pub *mut std::ffi::c_void);

#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NapiThreadsafeFunctionCallMode {
    Nonblocking = 0,
    Blocking = 1,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NapiThreadsafeFunctionReleaseMode {
    Release = 0,
    Abort = 1,
}

pub type NapiThreadsafeFunctionCallJs = unsafe extern "C" fn(
    env: napi_env,
    js_callback: napi_value,
    context: *mut std::ffi::c_void,
    data: *mut std::ffi::c_void,
);

pub type NapiFinalize = unsafe extern "C" fn(
    env: napi_env,
    finalize_data: *mut std::ffi::c_void,
    finalize_hint: *mut std::ffi::c_void,
);

#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NapiTypedArrayType {
    Int8 = 0,
    Uint8 = 1,
    Uint8Clamped = 2,
    Int16 = 3,
    Uint16 = 4,
    Int32 = 5,
    Uint32 = 6,
    Float32 = 7,
    Float64 = 8,
    BigInt64 = 9,
    BigUint64 = 10,
}
