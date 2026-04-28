//! Native-callback plumbing: bundling user `napi_callback`s into JS
//! functions, the V8 trampoline, and per-call `CallbackInfo`.

use std::ffi::c_void;

use v8::Global;

use crate::napi::env::NapiEnv;
use crate::napi::types::{NapiCallback, napi_callback_info, napi_env};

pub struct CallbackBundle {
    pub callback: NapiCallback,
    pub data: *mut c_void,
}

pub struct CallbackInfo<'s, 'a> {
    pub args: &'a v8::FunctionCallbackArguments<'s>,
    pub this: v8::Local<'s, v8::Value>,
    pub data: *mut c_void,
    pub new_target: Option<v8::Local<'s, v8::Value>>,
    pub return_value: Option<v8::Local<'s, v8::Value>>,
}

pub fn trampoline<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let data_value = args.data();
    let Ok(external) = v8::Local::<v8::External>::try_from(data_value) else {
        return;
    };
    let bundle: &CallbackBundle = unsafe { &*external.value().cast::<CallbackBundle>() };

    let context = scope.get_current_context();
    let context_global = Box::new(Global::new(scope, context));
    let context_ptr: *mut c_void = Box::into_raw(context_global).cast::<c_void>();
    let scope_ptr: *mut c_void =
        std::ptr::from_mut::<v8::PinScope<'s, '_>>(scope).cast::<c_void>();

    let mut env = NapiEnv::new(scope_ptr, context_ptr);
    let env_ptr: napi_env = std::ptr::from_mut(&mut env);

    let new_target = args.new_target();
    let info = CallbackInfo {
        args: &args,
        this: args.this().into(),
        data: bundle.data,
        new_target: if new_target.is_undefined() {
            None
        } else {
            Some(new_target)
        },
        return_value: None,
    };
    let info_ptr: napi_callback_info =
        napi_callback_info(std::ptr::from_ref(&info) as *mut c_void);

    let returned = unsafe { (bundle.callback)(env_ptr, info_ptr) };

    if let Some(g) = env.resolve(returned) {
        let local = v8::Local::new(scope, &g);
        rv.set(local);
    }

    drop(unsafe { Box::from_raw(context_ptr.cast::<Global<v8::Context>>()) });
}
