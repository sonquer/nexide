//! Installer that wires the `__nexide` op surface onto the V8 context.
//!
//! Every JavaScript op in `runtime/polyfills/*.js` reaches Rust
//! through `Nexide.core.ops.op_*`. Each op is a
//! [`v8::FunctionCallback`] that pulls the
//! [`super::bridge::BridgeStateHandle`] out of the isolate slot,
//! performs the work, and returns a `v8::Value`.

use bytes::Bytes;

use super::bridge::from_isolate;
use crate::ops::{DispatchTable, RequestId, RequestSource, ResponseHead, ResponseSink};

/// Installs `globalThis.Nexide.core.ops.{op_*}` and the small
/// `Nexide.core.{print, queueMicrotask, getAsyncContext,
/// setAsyncContext}` helper surface on `context`.
///
/// Must run after [`super::bridge::BridgeStateHandle`] has been parked
/// in the isolate slot - every op callback derefs that slot.
pub(super) fn install<'s>(scope: &mut v8::PinScope<'s, '_>, context: v8::Local<'s, v8::Context>) {
    let global = context.global(scope);
    let nexide_obj = v8::Object::new(scope);
    let core_obj = v8::Object::new(scope);
    let ops_obj = v8::Object::new(scope);

    install_ops(scope, ops_obj);
    install_core_helpers(scope, core_obj);
    set_property(scope, core_obj, "ops", ops_obj.into());
    set_property(scope, nexide_obj, "core", core_obj.into());
    set_property(scope, global, "Nexide", nexide_obj.into());
}

fn install_core_helpers<'s>(scope: &mut v8::PinScope<'s, '_>, core: v8::Local<'s, v8::Object>) {
    install_fn(scope, core, "print", op_core_print);
    install_fn(scope, core, "queueMicrotask", op_core_queue_microtask);
    install_fn(scope, core, "getAsyncContext", op_core_get_async_context);
    install_fn(scope, core, "setAsyncContext", op_core_set_async_context);
}

fn install_ops<'s>(scope: &mut v8::PinScope<'s, '_>, ops: v8::Local<'s, v8::Object>) {
    install_fn(scope, ops, "op_nexide_log", op_nexide_log);
    install_fn(scope, ops, "op_nexide_get_meta", op_nexide_get_meta);
    install_fn(scope, ops, "op_nexide_get_headers", op_nexide_get_headers);
    install_fn(scope, ops, "op_nexide_read_body", op_nexide_read_body);
    install_fn(scope, ops, "op_nexide_send_head", op_nexide_send_head);
    install_fn(scope, ops, "op_nexide_send_chunk", op_nexide_send_chunk);
    install_fn(scope, ops, "op_nexide_send_end", op_nexide_send_end);
    install_fn(
        scope,
        ops,
        "op_nexide_send_response",
        op_nexide_send_response,
    );
    install_fn(scope, ops, "op_nexide_finish_error", op_nexide_finish_error);
    install_fn(scope, ops, "op_nexide_pop_request", op_nexide_pop_request);
    install_fn(
        scope,
        ops,
        "op_nexide_pop_request_batch",
        op_nexide_pop_request_batch,
    );
    install_fn(
        scope,
        ops,
        "op_nexide_try_pop_request_batch",
        op_nexide_try_pop_request_batch,
    );
    install_fn(scope, ops, "op_now", op_now);
    install_fn(scope, ops, "op_void_async_deferred", op_void_async_deferred);
    install_fn(scope, ops, "op_timer_sleep", op_timer_sleep);
    install_fn(scope, ops, "op_process_meta", op_process_meta);
    install_fn(scope, ops, "op_process_env_get", op_process_env_get);
    install_fn(scope, ops, "op_process_env_has", op_process_env_has);
    install_fn(scope, ops, "op_process_env_keys", op_process_env_keys);
    install_fn(scope, ops, "op_process_env_set", op_process_env_set);
    install_fn(scope, ops, "op_process_env_delete", op_process_env_delete);
    install_fn(scope, ops, "op_process_hrtime_ns", op_process_hrtime_ns);
    install_fn(scope, ops, "op_process_exit", op_process_exit);
    install_fn(scope, ops, "op_process_kill", op_process_kill);
    install_fn(scope, ops, "op_process_cpu_usage", op_process_cpu_usage);
    install_fn(
        scope,
        ops,
        "op_process_memory_usage",
        op_process_memory_usage,
    );
    install_fn(scope, ops, "op_cjs_root_parent", op_cjs_root_parent);
    install_fn(scope, ops, "op_cjs_resolve", op_cjs_resolve);
    install_fn(scope, ops, "op_cjs_read_source", op_cjs_read_source);
    install_fn(
        scope,
        ops,
        "op_cjs_compile_function",
        op_cjs_compile_function,
    );
    install_fn(scope, ops, "op_napi_load", op_napi_load);
    install_fn(
        scope,
        ops,
        "op_esm_dynamic_import",
        super::esm::op_esm_dynamic_import,
    );

    install_fn(scope, ops, "op_os_arch", op_os_arch);
    install_fn(scope, ops, "op_os_platform", op_os_platform);
    install_fn(scope, ops, "op_os_type", op_os_type);
    install_fn(scope, ops, "op_os_release", op_os_release);
    install_fn(scope, ops, "op_os_hostname", op_os_hostname);
    install_fn(scope, ops, "op_os_tmpdir", op_os_tmpdir);
    install_fn(scope, ops, "op_os_homedir", op_os_homedir);
    install_fn(scope, ops, "op_os_endianness", op_os_endianness);
    install_fn(scope, ops, "op_os_uptime_secs", op_os_uptime_secs);
    install_fn(scope, ops, "op_os_freemem", op_os_freemem);
    install_fn(scope, ops, "op_os_totalmem", op_os_totalmem);
    install_fn(scope, ops, "op_os_cpus_count", op_os_cpus_count);

    install_fn(scope, ops, "op_fs_read", op_fs_read);
    install_fn(scope, ops, "op_fs_write", op_fs_write);
    install_fn(scope, ops, "op_fs_exists", op_fs_exists);
    install_fn(scope, ops, "op_fs_stat", op_fs_stat);
    install_fn(scope, ops, "op_fs_realpath", op_fs_realpath);
    install_fn(scope, ops, "op_fs_readdir", op_fs_readdir);
    install_fn(scope, ops, "op_fs_mkdir", op_fs_mkdir);
    install_fn(scope, ops, "op_fs_rm", op_fs_rm);
    install_fn(scope, ops, "op_fs_copy", op_fs_copy);
    install_fn(scope, ops, "op_fs_readlink", op_fs_readlink);

    install_fn(scope, ops, "op_crypto_hash", op_crypto_hash);
    install_fn(scope, ops, "op_crypto_hmac", op_crypto_hmac);
    install_fn(scope, ops, "op_crypto_random_bytes", op_crypto_random_bytes);
    install_fn(scope, ops, "op_crypto_random_uuid", op_crypto_random_uuid);
    install_fn(
        scope,
        ops,
        "op_crypto_timing_safe_equal",
        op_crypto_timing_safe_equal,
    );
    install_fn(scope, ops, "op_crypto_aes_gcm_seal", op_crypto_aes_gcm_seal);
    install_fn(scope, ops, "op_crypto_aes_gcm_open", op_crypto_aes_gcm_open);
    install_fn(scope, ops, "op_crypto_pbkdf2", op_crypto_pbkdf2);
    install_fn(scope, ops, "op_crypto_scrypt", op_crypto_scrypt);
    install_fn(scope, ops, "op_crypto_aes_encrypt", op_crypto_aes_encrypt);
    install_fn(scope, ops, "op_crypto_aes_decrypt", op_crypto_aes_decrypt);
    install_fn(
        scope,
        ops,
        "op_crypto_chacha20_seal",
        op_crypto_chacha20_seal,
    );
    install_fn(
        scope,
        ops,
        "op_crypto_chacha20_open",
        op_crypto_chacha20_open,
    );
    install_fn(scope, ops, "op_crypto_sign", op_crypto_sign);
    install_fn(scope, ops, "op_crypto_verify", op_crypto_verify);
    install_fn(scope, ops, "op_crypto_pem_decode", op_crypto_pem_decode);
    install_fn(scope, ops, "op_crypto_pem_encode", op_crypto_pem_encode);
    install_fn(
        scope,
        ops,
        "op_crypto_generate_key_pair",
        op_crypto_generate_key_pair,
    );
    install_fn(scope, ops, "op_crypto_key_inspect", op_crypto_key_inspect);
    install_fn(scope, ops, "op_crypto_key_convert", op_crypto_key_convert);
    install_fn(scope, ops, "op_crypto_jwk_to_der", op_crypto_jwk_to_der);
    install_fn(scope, ops, "op_crypto_der_to_jwk", op_crypto_der_to_jwk);
    install_fn(scope, ops, "op_crypto_rsa_encrypt", op_crypto_rsa_encrypt);
    install_fn(scope, ops, "op_crypto_rsa_decrypt", op_crypto_rsa_decrypt);
    install_fn(scope, ops, "op_crypto_sign_der", op_crypto_sign_der);
    install_fn(scope, ops, "op_crypto_verify_der", op_crypto_verify_der);
    install_fn(scope, ops, "op_crypto_ecdh_derive", op_crypto_ecdh_derive);
    install_fn(
        scope,
        ops,
        "op_crypto_x25519_derive",
        op_crypto_x25519_derive,
    );
    install_fn(
        scope,
        ops,
        "op_crypto_ecdh_generate",
        op_crypto_ecdh_generate,
    );
    install_fn(
        scope,
        ops,
        "op_crypto_ecdh_from_raw",
        op_crypto_ecdh_from_raw,
    );
    install_fn(
        scope,
        ops,
        "op_crypto_ecdh_compute_raw",
        op_crypto_ecdh_compute_raw,
    );
    install_fn(scope, ops, "op_crypto_hkdf", op_crypto_hkdf);

    install_fn(scope, ops, "op_zlib_encode", op_zlib_encode);
    install_fn(scope, ops, "op_zlib_decode", op_zlib_decode);

    install_fn(scope, ops, "op_dns_lookup", op_dns_lookup);
    install_fn(scope, ops, "op_dns_resolve4", op_dns_resolve4);
    install_fn(scope, ops, "op_dns_resolve6", op_dns_resolve6);
    install_fn(scope, ops, "op_dns_resolve_mx", op_dns_resolve_mx);
    install_fn(scope, ops, "op_dns_resolve_txt", op_dns_resolve_txt);
    install_fn(scope, ops, "op_dns_resolve_cname", op_dns_resolve_cname);
    install_fn(scope, ops, "op_dns_resolve_ns", op_dns_resolve_ns);
    install_fn(scope, ops, "op_dns_resolve_srv", op_dns_resolve_srv);
    install_fn(scope, ops, "op_dns_reverse", op_dns_reverse);

    install_fn(scope, ops, "op_net_connect", op_net_connect);
    install_fn(scope, ops, "op_net_listen", op_net_listen);
    install_fn(scope, ops, "op_net_accept", op_net_accept);
    install_fn(scope, ops, "op_net_read", op_net_read);
    install_fn(scope, ops, "op_net_write", op_net_write);
    install_fn(scope, ops, "op_net_close_stream", op_net_close_stream);
    install_fn(scope, ops, "op_net_close_listener", op_net_close_listener);
    install_fn(scope, ops, "op_net_set_nodelay", op_net_set_nodelay);
    install_fn(scope, ops, "op_net_set_keepalive", op_net_set_keepalive);

    install_fn(scope, ops, "op_tls_connect", op_tls_connect);
    install_fn(scope, ops, "op_tls_upgrade", op_tls_upgrade);
    install_fn(scope, ops, "op_tls_read", op_tls_read);
    install_fn(scope, ops, "op_tls_write", op_tls_write);
    install_fn(scope, ops, "op_tls_close", op_tls_close);

    install_fn(scope, ops, "op_http_request", op_http_request);
    install_fn(scope, ops, "op_http_response_read", op_http_response_read);
    install_fn(scope, ops, "op_http_response_close", op_http_response_close);

    install_fn(scope, ops, "op_proc_spawn", op_proc_spawn);
    install_fn(scope, ops, "op_proc_wait", op_proc_wait);
    install_fn(scope, ops, "op_proc_kill", op_proc_kill);
    install_fn(scope, ops, "op_proc_stdin_write", op_proc_stdin_write);
    install_fn(scope, ops, "op_proc_stdin_close", op_proc_stdin_close);
    install_fn(scope, ops, "op_proc_stdout_read", op_proc_stdout_read);
    install_fn(scope, ops, "op_proc_stderr_read", op_proc_stderr_read);
    install_fn(scope, ops, "op_proc_close", op_proc_close);

    install_fn(scope, ops, "op_zlib_create", op_zlib_create);
    install_fn(scope, ops, "op_zlib_feed", op_zlib_feed);
    install_fn(scope, ops, "op_zlib_finish", op_zlib_finish);
    install_fn(scope, ops, "op_zlib_close", op_zlib_close);

    install_fn(scope, ops, "op_vm_create_context", op_vm_create_context);
    install_fn(scope, ops, "op_vm_run_in_context", op_vm_run_in_context);
    install_fn(scope, ops, "op_vm_is_context", op_vm_is_context);
}

fn install_fn<'s, F>(
    scope: &mut v8::PinScope<'s, '_>,
    target: v8::Local<'s, v8::Object>,
    name: &str,
    callback: F,
) where
    F: v8::MapFnTo<v8::FunctionCallback>,
{
    let template = v8::FunctionTemplate::new(scope, callback);
    let function = template
        .get_function(scope)
        .expect("function template -> function");
    let key = v8::String::new(scope, name).expect("name string");
    target.set(scope, key.into(), function.into());
}

fn set_property<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    target: v8::Local<'s, v8::Object>,
    name: &str,
    value: v8::Local<'s, v8::Value>,
) {
    let key = v8::String::new(scope, name).expect("property name");
    target.set(scope, key.into(), value);
}

// ──────────────────────────────────────────────────────────────────────
// helpers
// ──────────────────────────────────────────────────────────────────────

fn request_id_from_args<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: &v8::FunctionCallbackArguments<'s>,
) -> Option<RequestId> {
    let idx = args.get(0).uint32_value(scope)?;
    let r#gen = args.get(1).uint32_value(scope)?;
    Some(RequestId::from_parts(idx, r#gen))
}

fn request_id_to_array<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    id: RequestId,
) -> v8::Local<'s, v8::Array> {
    let pair = v8::Array::new(scope, 2);
    let idx = v8::Number::new(scope, f64::from(id.index()));
    let r#gen = v8::Number::new(scope, f64::from(id.generation()));
    pair.set_index(scope, 0, idx.into());
    pair.set_index(scope, 1, r#gen.into());
    pair
}

fn parse_head<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    value: v8::Local<'s, v8::Value>,
) -> Option<ResponseHead> {
    let obj: v8::Local<v8::Object> = value.try_into().ok()?;
    let status_key = v8::String::new(scope, "status")?;
    let status = obj.get(scope, status_key.into())?.uint32_value(scope)?;
    let headers_key = v8::String::new(scope, "headers")?;
    let raw_headers = obj.get(scope, headers_key.into())?;
    let headers = if raw_headers.is_array() {
        let arr: v8::Local<v8::Array> = raw_headers.try_into().ok()?;
        let len = arr.length();
        let mut out = Vec::with_capacity(len as usize);
        for i in 0..len {
            let pair_val = arr.get_index(scope, i)?;
            let pair: v8::Local<v8::Array> = pair_val.try_into().ok()?;
            let name = pair.get_index(scope, 0)?.to_rust_string_lossy(scope);
            let value = pair.get_index(scope, 1)?.to_rust_string_lossy(scope);
            out.push((name, value));
        }
        out
    } else {
        Vec::new()
    };
    Some(ResponseHead {
        status: u16::try_from(status).ok()?,
        headers,
    })
}

fn read_bytes_arg<'s>(
    _scope: &mut v8::PinScope<'s, '_>,
    value: v8::Local<'s, v8::Value>,
) -> Option<Bytes> {
    if let Ok(view) = TryInto::<v8::Local<v8::Uint8Array>>::try_into(value) {
        let len = view.byte_length();
        if len == 0 {
            return Some(Bytes::new());
        }
        let mut buf: Vec<u8> = Vec::with_capacity(len);
        unsafe {
            let slice = std::slice::from_raw_parts_mut(buf.as_mut_ptr(), len);
            let copied = view.copy_contents(slice);
            buf.set_len(copied);
        }
        return Some(Bytes::from(buf));
    }
    if let Ok(buf) = TryInto::<v8::Local<v8::ArrayBuffer>>::try_into(value) {
        let store = buf.get_backing_store();
        let len = store.byte_length();
        if len == 0 {
            return Some(Bytes::new());
        }
        if let Some(data) = store.data() {
            let raw = unsafe { std::slice::from_raw_parts(data.as_ptr() as *const u8, len) };
            return Some(Bytes::copy_from_slice(raw));
        }
        return Some(Bytes::new());
    }
    None
}

fn throw_error<'s>(scope: &mut v8::PinScope<'s, '_>, message: &str) {
    let msg = v8::String::new(scope, message).unwrap();
    let exception = v8::Exception::error(scope, msg);
    scope.throw_exception(exception);
}

fn throw_type_error<'s>(scope: &mut v8::PinScope<'s, '_>, message: &str) {
    let msg = v8::String::new(scope, message).unwrap();
    let exception = v8::Exception::type_error(scope, msg);
    scope.throw_exception(exception);
}

/// Settles a request: takes the response payload, fires the
/// completion oneshot with `Ok(payload)`, and removes the slot.
fn settle_ok(table: &mut DispatchTable, id: RequestId) -> Result<(), String> {
    let inflight = table.get_mut(id).map_err(|e| e.to_string())?;
    let response = std::mem::take(inflight.response_mut());
    let payload = response.finish().map_err(|e| e.to_string())?;
    if let Some(tx) = inflight.take_completion() {
        let _ = tx.send(Ok(payload));
    }
    let _ = table.remove(id);
    Ok(())
}

/// Settles a request with a handler error.
fn settle_err(table: &mut DispatchTable, id: RequestId, msg: &str) -> Result<(), String> {
    let inflight = table.get_mut(id).map_err(|e| e.to_string())?;
    if let Some(tx) = inflight.take_completion() {
        let _ = tx.send(Err(crate::ops::RequestFailure::Handler(msg.to_owned())));
    }
    let _ = table.remove(id);
    Ok(())
}

// ──────────────────────────────────────────────────────────────────────
// op implementations
// ──────────────────────────────────────────────────────────────────────

fn op_core_print<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    if args.length() == 0 {
        rv.set_undefined();
        return;
    }
    let msg = args.get(0).to_rust_string_lossy(scope);
    let is_err = args.length() >= 2 && args.get(1).boolean_value(scope);
    let is_primary = from_isolate(scope).0.borrow().worker_id.is_primary;
    if is_err {
        tracing::error!(target: "nexide::js::print", "{msg}");
    } else if is_primary {
        tracing::info!(target: "nexide::js::print", "{msg}");
    }
    rv.set_undefined();
}

fn op_core_queue_microtask<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    if let Ok(cb) = TryInto::<v8::Local<v8::Function>>::try_into(args.get(0)) {
        scope.enqueue_microtask(cb);
    }
    rv.set_undefined();
}

/// Returns the V8 Continuation-Preserved Embedder Data - the value
/// stored on the active promise continuation, automatically propagated
/// across `await`, `.then()`, `queueMicrotask()`, and timer
/// resumptions. Used by [`AsyncLocalStorage`] to carry per-request
/// stores across async boundaries.
fn op_core_get_async_context<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let value = scope.get_continuation_preserved_embedder_data();
    rv.set(value);
}

/// Stores `args[0]` on the active continuation as the
/// Continuation-Preserved Embedder Data, replacing any previous value.
/// Subsequent async resumptions observe the new snapshot until another
/// `setAsyncContext` overrides it.
fn op_core_set_async_context<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let value = args.get(0);
    scope.set_continuation_preserved_embedder_data(value);
    rv.set_undefined();
}

fn op_now<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64() * 1000.0);
    let value = v8::Number::new(scope, ms);
    rv.set(value.into());
}

fn op_nexide_log<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let level = args.get(0).uint32_value(scope).unwrap_or(2);
    let msg = args.get(1).to_rust_string_lossy(scope);
    let handle = from_isolate(scope);
    let worker_id = handle.0.borrow().worker_id;
    match level {
        0 => {
            if worker_id.is_primary {
                tracing::trace!(target: "nexide::js", worker = worker_id.id, "{msg}");
            }
        }
        1 => {
            if worker_id.is_primary {
                tracing::debug!(target: "nexide::js", worker = worker_id.id, "{msg}");
            }
        }
        2 => {
            if worker_id.is_primary {
                tracing::info!(target: "nexide::js", worker = worker_id.id, "{msg}");
            }
        }
        3 => tracing::warn!(target: "nexide::js", worker = worker_id.id, "{msg}"),
        _ => tracing::error!(target: "nexide::js", worker = worker_id.id, "{msg}"),
    }
    rv.set_undefined();
}

fn op_nexide_get_meta<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Some(id) = request_id_from_args(scope, &args) else {
        throw_type_error(scope, "op_nexide_get_meta: expected (idx, gen)");
        return;
    };
    let handle = from_isolate(scope);
    let (method, uri) = {
        let state = handle.0.borrow();
        match state.dispatch_table.get(id) {
            Ok(slot) => {
                let m = slot.request().meta();
                (m.method().to_owned(), m.uri().to_owned())
            }
            Err(err) => {
                let msg = err.to_string();
                drop(state);
                throw_error(scope, &msg);
                return;
            }
        }
    };
    // Hot-path optimisation: HTTP method (RFC 7230 token) and URI
    // (RFC 3986 ASCII) are always one-byte; `new_from_one_byte`
    // bypasses V8's UTF-8 → UTF-16 transcoding path (typical 2-3×
    // faster for short strings). Layout switched from
    // `{ method, uri }` to `[method, uri]`: saves one `v8::Object`
    // allocation, two property `Set` calls, and the hidden-class
    // transition per request. JS side reads `meta[0]`/`meta[1]`.
    let m_val = ascii_v8_string(scope, method.as_bytes());
    let u_val = ascii_v8_string(scope, uri.as_bytes());
    let array = v8::Array::new(scope, 2);
    array.set_index(scope, 0, m_val.into());
    array.set_index(scope, 1, u_val.into());
    rv.set(array.into());
}

fn op_nexide_get_headers<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Some(id) = request_id_from_args(scope, &args) else {
        throw_type_error(scope, "op_nexide_get_headers: expected (idx, gen)");
        return;
    };
    let handle = from_isolate(scope);
    let headers: Vec<(String, String)> = {
        let state = handle.0.borrow();
        match state.dispatch_table.get(id) {
            Ok(slot) => slot
                .request()
                .headers()
                .iter()
                .map(|h| (h.name.clone(), h.value.clone()))
                .collect(),
            Err(err) => {
                let msg = err.to_string();
                drop(state);
                throw_error(scope, &msg);
                return;
            }
        }
    };
    // Hot-path optimisation: returns a *flat* `[name, value, name,
    // value, ...]` array instead of an array of `{ name, value }`
    // objects. This eliminates one `v8::Object` allocation and two
    // property `Set` calls per header (typical request: ~15 headers
    // → 15 fewer object allocations + 30 fewer hidden-class
    // transitions). Combined with the ASCII fast-path
    // (`new_from_one_byte`, bypasses UTF-8 → UTF-16 transcoding for
    // header names+values which `HeaderValue::to_str` already
    // guarantees are visible ASCII), this is one of the heaviest
    // per-request bridge calls. JS side iterates by stride-2.
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    let array = v8::Array::new(scope, (headers.len() * 2) as i32);
    for (i, (name, value)) in headers.into_iter().enumerate() {
        let n = ascii_v8_string(scope, name.as_bytes());
        let v = ascii_v8_string(scope, value.as_bytes());
        #[allow(clippy::cast_possible_truncation)]
        let base = (i * 2) as u32;
        array.set_index(scope, base, n.into());
        array.set_index(scope, base + 1, v.into());
    }
    rv.set(array.into());
}

/// Allocates a V8 string from an ASCII byte slice using the one-byte
/// fast path.
///
/// `v8::String::new_from_one_byte` skips the UTF-8 → UTF-16
/// transcoding step that `v8::String::new` (UTF-8) always pays. For
/// HTTP traffic - method/URI/header names+values - the bytes are
/// guaranteed visible ASCII (method is a token per RFC 7230, URI is
/// ASCII per RFC 3986, header values that survived
/// `HeaderValue::to_str` are visible ASCII), so this is always safe
/// and measurably faster on hot paths. Falls back to an empty string
/// only on the V8-internal length overflow case (effectively
/// unreachable for sane HTTP).
fn ascii_v8_string<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    bytes: &[u8],
) -> v8::Local<'s, v8::String> {
    v8::String::new_from_one_byte(scope, bytes, v8::NewStringType::Normal)
        .unwrap_or_else(|| v8::String::empty(scope))
}

fn op_nexide_read_body<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Some(id) = request_id_from_args(scope, &args) else {
        throw_type_error(scope, "op_nexide_read_body: expected (idx, gen, buf)");
        return;
    };
    let buf_value = args.get(2);
    let view: v8::Local<v8::Uint8Array> = match buf_value.try_into() {
        Ok(v) => v,
        Err(_) => {
            throw_type_error(scope, "op_nexide_read_body: dst must be Uint8Array");
            return;
        }
    };
    let len = view.byte_length();
    let mut tmp = vec![0u8; len];
    let handle = from_isolate(scope);
    let written = {
        let mut state = handle.0.borrow_mut();
        match state.dispatch_table.get_mut(id) {
            Ok(slot) => slot.request_mut().read_body(&mut tmp),
            Err(err) => {
                let msg = err.to_string();
                drop(state);
                throw_error(scope, &msg);
                return;
            }
        }
    };
    if written > 0 {
        let backing = view.buffer(scope).unwrap().get_backing_store();
        if let Some(data) = backing.data() {
            unsafe {
                let dst = data.as_ptr() as *mut u8;
                std::ptr::copy_nonoverlapping(tmp.as_ptr(), dst.add(view.byte_offset()), written);
            }
        }
    }
    rv.set_uint32(u32::try_from(written).unwrap_or(u32::MAX));
}

fn op_nexide_send_head<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Some(id) = request_id_from_args(scope, &args) else {
        throw_type_error(scope, "op_nexide_send_head: expected (idx, gen, head)");
        return;
    };
    let head_value = args.get(2);
    let Some(head) = parse_head(scope, head_value) else {
        throw_type_error(scope, "op_nexide_send_head: invalid head shape");
        return;
    };
    let handle = from_isolate(scope);
    let result = {
        let mut state = handle.0.borrow_mut();
        match state.dispatch_table.get_mut(id) {
            Ok(slot) => slot
                .response_mut()
                .send_head(head)
                .map_err(|e| e.to_string()),
            Err(err) => Err(err.to_string()),
        }
    };
    if let Err(msg) = result {
        throw_error(scope, &msg);
        return;
    }
    rv.set_undefined();
}

fn op_nexide_send_chunk<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Some(id) = request_id_from_args(scope, &args) else {
        throw_type_error(scope, "op_nexide_send_chunk: expected (idx, gen, bytes)");
        return;
    };
    let Some(bytes) = read_bytes_arg(scope, args.get(2)) else {
        throw_type_error(scope, "op_nexide_send_chunk: bytes must be Uint8Array");
        return;
    };
    let handle = from_isolate(scope);
    let result = {
        let mut state = handle.0.borrow_mut();
        match state.dispatch_table.get_mut(id) {
            Ok(slot) => slot
                .response_mut()
                .send_chunk(bytes)
                .map_err(|e| e.to_string()),
            Err(err) => Err(err.to_string()),
        }
    };
    if let Err(msg) = result {
        throw_error(scope, &msg);
        return;
    }
    rv.set_undefined();
}

fn op_nexide_send_end<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Some(id) = request_id_from_args(scope, &args) else {
        throw_type_error(scope, "op_nexide_send_end: expected (idx, gen)");
        return;
    };
    let handle = from_isolate(scope);
    let result = settle_ok(&mut handle.0.borrow_mut().dispatch_table, id);
    if let Err(msg) = result {
        throw_error(scope, &msg);
        return;
    }
    rv.set_undefined();
}

fn op_nexide_send_response<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Some(id) = request_id_from_args(scope, &args) else {
        throw_type_error(
            scope,
            "op_nexide_send_response: expected (idx, gen, head, body)",
        );
        return;
    };
    let Some(head) = parse_head(scope, args.get(2)) else {
        throw_type_error(scope, "op_nexide_send_response: invalid head shape");
        return;
    };
    let body = read_bytes_arg(scope, args.get(3)).unwrap_or_default();
    let handle = from_isolate(scope);
    let result = (|| -> Result<(), String> {
        let mut state = handle.0.borrow_mut();
        let table = &mut state.dispatch_table;
        let inflight = table.get_mut(id).map_err(|e| e.to_string())?;
        let response = inflight.response_mut();
        response.send_head(head).map_err(|e| e.to_string())?;
        if !body.is_empty() {
            response.send_chunk(body).map_err(|e| e.to_string())?;
        }
        drop(state);
        settle_ok(&mut handle.0.borrow_mut().dispatch_table, id)
    })();
    if let Err(msg) = result {
        throw_error(scope, &msg);
        return;
    }
    rv.set_undefined();
}

fn op_nexide_finish_error<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Some(id) = request_id_from_args(scope, &args) else {
        throw_type_error(scope, "op_nexide_finish_error: expected (idx, gen, msg)");
        return;
    };
    let msg = args.get(2).to_rust_string_lossy(scope);
    let handle = from_isolate(scope);
    let _ = settle_err(&mut handle.0.borrow_mut().dispatch_table, id, &msg);
    rv.set_undefined();
}

fn op_nexide_pop_request<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let resolver = match v8::PromiseResolver::new(scope) {
        Some(r) => r,
        None => {
            rv.set_undefined();
            return;
        }
    };
    let promise = resolver.get_promise(scope);
    let handle = from_isolate(scope);
    let popped = handle.0.borrow().queue.try_pop_batch(1);
    match popped.into_iter().next() {
        Some(id) => {
            let pair = request_id_to_array(scope, id);
            resolver.resolve(scope, pair.into());
        }
        None => {
            let global = v8::Global::new(scope, resolver);
            handle.0.borrow_mut().pending_pop.push_back(global);
        }
    }
    rv.set(promise.into());
}

fn op_nexide_pop_request_batch<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let max = args.get(0).uint32_value(scope).unwrap_or(16).max(1);
    let resolver = match v8::PromiseResolver::new(scope) {
        Some(r) => r,
        None => {
            rv.set_undefined();
            return;
        }
    };
    let promise = resolver.get_promise(scope);
    let handle = from_isolate(scope);
    let ids = handle.0.borrow().queue.try_pop_batch(max as usize);
    if ids.is_empty() {
        let global = v8::Global::new(scope, resolver);
        handle
            .0
            .borrow_mut()
            .pending_pop_batch
            .push_back((global, max));
    } else {
        let array = v8::Array::new(scope, ids.len() as i32);
        for (i, id) in ids.into_iter().enumerate() {
            let pair = request_id_to_array(scope, id);
            array.set_index(scope, i as u32, pair.into());
        }
        resolver.resolve(scope, array.into());
    }
    rv.set(promise.into());
}

/// Resolves any pending `op_nexide_pop_request{,_batch}` promises whose
/// queue has at least one ready request. Called by the engine pump
/// before each microtask checkpoint so JS pump loops can wake up.
pub(super) fn drain_pending_pops<'s>(scope: &mut v8::PinScope<'s, '_>) {
    loop {
        let handle = from_isolate(scope);
        let mut state = handle.0.borrow_mut();

        if !state.pending_pop.is_empty() {
            let popped = state.queue.try_pop_batch(1);
            let Some(id) = popped.into_iter().next() else {
                return;
            };
            let resolver_global = state.pending_pop.pop_front().unwrap();
            drop(state);
            let resolver = v8::Local::new(scope, &resolver_global);
            let pair = request_id_to_array(scope, id);
            resolver.resolve(scope, pair.into());
            continue;
        }

        if let Some(cap) = state.pending_pop_batch.front().map(|(_, c)| *c) {
            let popped = state.queue.try_pop_batch(cap as usize);
            if popped.is_empty() {
                return;
            }
            let (resolver_global, _) = state.pending_pop_batch.pop_front().unwrap();
            drop(state);
            let resolver = v8::Local::new(scope, &resolver_global);
            let arr = v8::Array::new(scope, popped.len() as i32);
            for (i, id) in popped.iter().enumerate() {
                let pair = request_id_to_array(scope, *id);
                arr.set_index(scope, i as u32, pair.into());
            }
            resolver.resolve(scope, arr.into());
            continue;
        }

        return;
    }
}

fn op_nexide_try_pop_request_batch<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let max = args.get(0).uint32_value(scope).unwrap_or(16).max(1) as usize;
    let handle = from_isolate(scope);
    let ids = handle.0.borrow().queue.try_pop_batch(max);
    let array = v8::Array::new(scope, ids.len() as i32);
    for (i, id) in ids.into_iter().enumerate() {
        let pair = request_id_to_array(scope, id);
        array.set_index(scope, i as u32, pair.into());
    }
    rv.set(array.into());
}

fn op_void_async_deferred<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let resolver = match v8::PromiseResolver::new(scope) {
        Some(r) => r,
        None => {
            rv.set_undefined();
            return;
        }
    };
    let undef = v8::undefined(scope);
    resolver.resolve(scope, undef.into());
    rv.set(resolver.get_promise(scope).into());
}

fn op_process_meta<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let handle = from_isolate(scope);
    let state = handle.0.borrow();
    let cwd = state
        .process
        .as_ref()
        .map(|p| p.cwd().to_owned())
        .unwrap_or_else(|| {
            std::env::current_dir()
                .ok()
                .and_then(|p| p.into_os_string().into_string().ok())
                .unwrap_or_default()
        });
    let platform = state
        .process
        .as_ref()
        .map_or_else(host_platform_str, |p| p.platform());
    let arch = state
        .process
        .as_ref()
        .map_or_else(host_arch_str, |p| p.arch());
    drop(state);

    let obj = v8::Object::new(scope);
    set_string_field(scope, obj, "platform", platform);
    set_string_field(scope, obj, "arch", arch);
    set_string_field(scope, obj, "cwd", &cwd);
    set_string_field(scope, obj, "version", "v22.0.0");
    let pid = v8::Number::new(scope, f64::from(std::process::id()));
    let pid_key = v8::String::new(scope, "pid").unwrap();
    obj.set(scope, pid_key.into(), pid.into());
    let argv = v8::Array::new(scope, 0);
    let argv_key = v8::String::new(scope, "argv").unwrap();
    obj.set(scope, argv_key.into(), argv.into());
    rv.set(obj.into());
}

fn host_platform_str() -> &'static str {
    if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "windows") {
        "win32"
    } else {
        "unknown"
    }
}

fn host_arch_str() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "x64"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "unknown"
    }
}

fn set_string_field<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    obj: v8::Local<'s, v8::Object>,
    name: &str,
    value: &str,
) {
    let key = v8::String::new(scope, name).unwrap();
    let val = v8::String::new(scope, value).unwrap();
    obj.set(scope, key.into(), val.into());
}

fn op_process_env_get<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let key = args.get(0).to_rust_string_lossy(scope);
    let handle = from_isolate(scope);
    let state = handle.0.borrow();
    if let Some(overlay) = state.env_overlay.lookup(&key) {
        match overlay {
            Some(v) => {
                let s = v8::String::new(scope, &v).unwrap();
                rv.set(s.into());
            }
            None => rv.set_null(),
        }
        return;
    }
    let value = state.process.as_ref().and_then(|p| p.get(&key));
    drop(state);
    match value {
        Some(v) => {
            let s = v8::String::new(scope, &v).unwrap();
            rv.set(s.into());
        }
        None => rv.set_null(),
    }
}

fn op_process_env_has<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let key = args.get(0).to_rust_string_lossy(scope);
    let handle = from_isolate(scope);
    let state = handle.0.borrow();
    let present = match state.env_overlay.lookup(&key) {
        Some(Some(_)) => true,
        Some(None) => false,
        None => state
            .process
            .as_ref()
            .is_some_and(|p| p.get(&key).is_some()),
    };
    rv.set_bool(present);
}

fn op_process_env_keys<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let handle = from_isolate(scope);
    let state = handle.0.borrow();
    let mut keys: std::collections::BTreeSet<String> = state
        .process
        .as_ref()
        .map(|p| p.visible_keys().into_iter().collect())
        .unwrap_or_default();
    for (k, _) in state.env_overlay.live_entries() {
        keys.insert(k);
    }
    for k in state.env_overlay.deleted_keys() {
        keys.remove(&k);
    }
    drop(state);
    let arr = v8::Array::new(scope, keys.len() as i32);
    for (i, k) in keys.iter().enumerate() {
        let s = v8::String::new(scope, k).unwrap();
        arr.set_index(scope, i as u32, s.into());
    }
    rv.set(arr.into());
}

fn op_process_env_set<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let key = args.get(0).to_rust_string_lossy(scope);
    let value = args.get(1).to_rust_string_lossy(scope);
    if key.is_empty() || key.contains('=') || key.contains('\0') {
        throw_error(
            scope,
            "ERR_INVALID_ARG_VALUE: env key must be non-empty and contain neither '=' nor NUL",
        );
        return;
    }
    if value.contains('\0') {
        throw_error(
            scope,
            "ERR_INVALID_ARG_VALUE: env value must not contain NUL bytes",
        );
        return;
    }
    let handle = from_isolate(scope);
    handle.0.borrow().env_overlay.set(key, value);
    rv.set_undefined();
}

fn op_process_env_delete<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let key = args.get(0).to_rust_string_lossy(scope);
    if key.is_empty() || key.contains('=') || key.contains('\0') {
        throw_error(
            scope,
            "ERR_INVALID_ARG_VALUE: env key must be non-empty and contain neither '=' nor NUL",
        );
        return;
    }
    let handle = from_isolate(scope);
    handle.0.borrow().env_overlay.delete(key);
    rv.set_undefined();
}

fn op_process_hrtime_ns<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let handle = from_isolate(scope);
    let ns = handle
        .0
        .borrow()
        .process
        .as_ref()
        .map_or(0u64, ProcessConfigClock::hrtime_ns);
    let big = v8::BigInt::new_from_u64(scope, ns);
    rv.set(big.into());
}

trait ProcessConfigClock {
    fn hrtime_ns(&self) -> u64;
}

impl ProcessConfigClock for crate::ops::ProcessConfig {
    fn hrtime_ns(&self) -> u64 {
        crate::ops::ProcessConfig::hrtime_ns(self)
    }
}

fn op_process_exit<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let raw = args.get(0).int32_value(scope).unwrap_or(0);
    // Node clamps exit codes to 0..=255 (POSIX waitstatus). Negative or
    // out-of-range values flow into a benign 1 instead of corrupting the
    // signed representation we expose to embedders.
    let code = if (0..=255).contains(&raw) { raw } else { 1 };
    let handle = from_isolate(scope);
    handle.0.borrow_mut().exit_requested = Some(code);
    rv.set_undefined();
}

/// `process.kill(pid, signum)` - gated on
/// [`ProcessConfig::subprocess_allowed`] because the syscall reaches
/// arbitrary host PIDs. Returns `true` on success, throws
/// Node-shaped errors on failure.
fn op_process_kill<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let pid = args.get(0).int32_value(scope).unwrap_or(0);
    let signum = args.get(1).int32_value(scope).unwrap_or(15);
    let handle = from_isolate(scope);
    {
        let state = handle.0.borrow();
        let allowed = state
            .process
            .as_ref()
            .is_none_or(crate::ops::ProcessConfig::subprocess_allowed);
        if !allowed {
            drop(state);
            let err = NetError::new("EPERM", "process.kill is disabled by ProcessConfig");
            let exc = make_node_error(scope, &err);
            scope.throw_exception(exc);
            return;
        }
    }
    if !(0..=64).contains(&signum) {
        let err = NetError::new(
            "ERR_INVALID_ARG_VALUE",
            format!("signal {signum} is outside the supported 0..=64 range"),
        );
        let exc = make_node_error(scope, &err);
        scope.throw_exception(exc);
        return;
    }
    #[cfg(unix)]
    {
        let res = unsafe { libc::kill(pid, signum) };
        if res != 0 {
            let io = std::io::Error::last_os_error();
            let code = match io.raw_os_error() {
                Some(libc::ESRCH) => "ESRCH",
                Some(libc::EPERM) => "EPERM",
                Some(libc::EINVAL) => "EINVAL",
                _ => "EIO",
            };
            let err = NetError::new(code, io.to_string());
            let exc = make_node_error(scope, &err);
            scope.throw_exception(exc);
            return;
        }
        rv.set(v8::Boolean::new(scope, true).into());
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        let err = NetError::new("ENOSYS", "process.kill is not supported on this platform");
        let exc = make_node_error(scope, &err);
        scope.throw_exception(exc);
    }
}

/// `process.cpuUsage()` - returns `{ user, system }` in microseconds.
/// Unix uses `getrusage(RUSAGE_SELF)`; other platforms return zeroes.
fn op_process_cpu_usage<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let (user, sys) = cpu_usage_micros();
    let obj = v8::Object::new(scope);
    let user_key = v8::String::new(scope, "user").unwrap();
    let user_val = v8::Number::new(scope, user as f64);
    obj.set(scope, user_key.into(), user_val.into());
    let sys_key = v8::String::new(scope, "system").unwrap();
    let sys_val = v8::Number::new(scope, sys as f64);
    obj.set(scope, sys_key.into(), sys_val.into());
    rv.set(obj.into());
}

#[cfg(unix)]
fn cpu_usage_micros() -> (u64, u64) {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
    if rc != 0 {
        return (0, 0);
    }
    #[allow(clippy::useless_conversion)] // tv_usec is i32 on macOS, i64 on Linux
    let to_micros = |t: libc::timeval| {
        (t.tv_sec)
            .saturating_mul(1_000_000)
            .saturating_add(i64::from(t.tv_usec))
    };
    let user = to_micros(usage.ru_utime).max(0) as u64;
    let sys = to_micros(usage.ru_stime).max(0) as u64;
    (user, sys)
}

#[cfg(not(unix))]
fn cpu_usage_micros() -> (u64, u64) {
    (0, 0)
}

/// `process.memoryUsage()` - best-effort RSS via the `sysinfo` crate.
/// `heapTotal`, `heapUsed`, `external`, and `arrayBuffers` are reported
/// as zero (V8 does not expose these stats through the public op
/// surface yet).
fn op_process_memory_usage<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let rss = current_rss_bytes();
    let obj = v8::Object::new(scope);
    for (name, value) in [
        ("rss", rss),
        ("heapTotal", 0),
        ("heapUsed", 0),
        ("external", 0),
        ("arrayBuffers", 0),
    ] {
        let key = v8::String::new(scope, name).unwrap();
        let val = v8::Number::new(scope, value as f64);
        obj.set(scope, key.into(), val.into());
    }
    rv.set(obj.into());
}

fn current_rss_bytes() -> u64 {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};
    let mut sys = System::new_with_specifics(
        RefreshKind::nothing().with_processes(ProcessRefreshKind::nothing().with_memory()),
    );
    let pid = Pid::from_u32(std::process::id());
    sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    sys.process(pid).map(sysinfo::Process::memory).unwrap_or(0)
}

fn op_cjs_root_parent<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let handle = from_isolate(scope);
    let root = handle.0.borrow().cjs_root.clone();
    let s = v8::String::new(scope, &root).unwrap();
    rv.set(s.into());
}

fn op_cjs_resolve<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let parent = args.get(0).to_rust_string_lossy(scope);
    let request = args.get(1).to_rust_string_lossy(scope);
    let bad = |s: &str| s.chars().any(|c| c == '\n' || c == '\r' || c == '\0');
    if bad(&parent) || bad(&request) {
        throw_error(scope, "EINVAL: invalid module specifier");
        return;
    }
    let handle = from_isolate(scope);
    let resolver = handle.0.borrow().cjs.clone();
    let Some(resolver) = resolver else {
        throw_error(scope, "cjs resolver not configured");
        return;
    };
    match resolver.resolve(&parent, &request) {
        Ok(resolved) => {
            let s = v8::String::new(scope, &resolved.to_specifier()).unwrap();
            rv.set(s.into());
        }
        Err(err) => throw_error(scope, &err.to_string()),
    }
}

fn op_cjs_read_source<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let specifier = args.get(0).to_rust_string_lossy(scope);
    if specifier.is_empty()
        || specifier
            .chars()
            .any(|c| c == '\n' || c == '\r' || c == '\0')
    {
        throw_error(scope, "EINVAL: invalid module specifier");
        return;
    }
    let resolved = match crate::engine::cjs::Resolved::from_specifier(&specifier) {
        Ok(r) => r,
        Err(err) => {
            throw_error(scope, &err.to_string());
            return;
        }
    };
    let resolver = {
        let handle = from_isolate(scope);
        let cjs = handle.0.borrow().cjs.clone();
        match cjs {
            Some(r) => r,
            None => {
                throw_error(scope, "cjs resolver not configured");
                return;
            }
        }
    };
    let (kind_int, source) = match resolved {
        crate::engine::cjs::Resolved::Builtin(name) => match resolver.builtin_source(&name) {
            Some(src) => (2u32, src.to_owned()),
            None => {
                throw_error(scope, &format!("node:{name} not registered"));
                return;
            }
        },
        crate::engine::cjs::Resolved::File(path) => {
            if !resolver.is_path_admitted(path.as_path()) {
                throw_error(
                    scope,
                    &format!("EACCES: read denied for '{}'", path.display()),
                );
                return;
            }
            match std::fs::read_to_string(&path) {
                Ok(text) => (0u32, text),
                Err(err) => {
                    throw_error(scope, &format!("read failed: {err}"));
                    return;
                }
            }
        }
        crate::engine::cjs::Resolved::Json(path) => {
            if !resolver.is_path_admitted(path.as_path()) {
                throw_error(
                    scope,
                    &format!("EACCES: read denied for '{}'", path.display()),
                );
                return;
            }
            match std::fs::read_to_string(&path) {
                Ok(text) => (1u32, text),
                Err(err) => {
                    throw_error(scope, &format!("read failed: {err}"));
                    return;
                }
            }
        }
        crate::engine::cjs::Resolved::Native(path) => {
            if !resolver.is_path_admitted(path.as_path()) {
                throw_error(
                    scope,
                    &format!("EACCES: read denied for '{}'", path.display()),
                );
                return;
            }
            (3u32, path.to_string_lossy().into_owned())
        }
    };
    let arr = v8::Array::new(scope, 2);
    let src_str = v8::String::new(scope, &source).unwrap();
    let kind_num = v8::Number::new(scope, f64::from(kind_int));
    arr.set_index(scope, 0, src_str.into());
    arr.set_index(scope, 1, kind_num.into());
    rv.set(arr.into());
}

fn op_cjs_compile_function<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let source = args.get(0).to_rust_string_lossy(scope);
    let specifier = args.get(1).to_rust_string_lossy(scope);
    if specifier.is_empty()
        || specifier
            .chars()
            .any(|c| c == '\n' || c == '\r' || c == '\0')
    {
        throw_error(scope, "EINVAL: invalid module specifier");
        return;
    }
    let Some(code_str) = v8::String::new(scope, &source) else {
        throw_error(scope, "op_cjs_compile_function: failed to allocate source");
        return;
    };
    let Some(resource) = v8::String::new(scope, &specifier) else {
        throw_error(
            scope,
            "op_cjs_compile_function: failed to allocate specifier",
        );
        return;
    };
    let undefined = v8::undefined(scope).into();
    let origin = v8::ScriptOrigin::new(
        scope,
        resource.into(),
        0,
        0,
        false,
        0,
        Some(undefined),
        false,
        false,
        false,
        None,
    );

    let cache = super::engine::code_cache_from_isolate(scope);
    let cached_bytes = cache
        .as_ref()
        .filter(|c| c.is_enabled())
        .and_then(|c| c.lookup(&source));

    let (mut src_obj, options) = match cached_bytes {
        Some(bytes) => {
            let cached = v8::script_compiler::CachedData::new(&bytes);
            (
                v8::script_compiler::Source::new_with_cached_data(code_str, Some(&origin), cached),
                v8::script_compiler::CompileOptions::ConsumeCodeCache,
            )
        }
        None => (
            v8::script_compiler::Source::new(code_str, Some(&origin)),
            v8::script_compiler::CompileOptions::NoCompileOptions,
        ),
    };

    let arg_names = [
        v8::String::new(scope, "exports").unwrap(),
        v8::String::new(scope, "require").unwrap(),
        v8::String::new(scope, "module").unwrap(),
        v8::String::new(scope, "__filename").unwrap(),
        v8::String::new(scope, "__dirname").unwrap(),
    ];
    let func = match v8::script_compiler::compile_function(
        scope,
        &mut src_obj,
        &arg_names,
        &[],
        options,
        v8::script_compiler::NoCacheReason::NoReason,
    ) {
        Some(f) => f,
        None => return,
    };

    if let Some(cache) = cache.as_ref().filter(|c| c.is_enabled()) {
        let consumed = options.contains(v8::script_compiler::CompileOptions::ConsumeCodeCache);
        let rejected = src_obj
            .get_cached_data()
            .map(v8::CachedData::rejected)
            .unwrap_or(false);

        if !consumed || rejected {
            if consumed {
                cache.metrics().record_reject();
            }
            if let Some(blob) = func.create_code_cache() {
                let bytes = blob.to_vec();
                if !bytes.is_empty() {
                    cache.store(&source, bytes);
                }
            }
        }
    }

    rv.set(func.into());
}

fn op_napi_load<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let path = args.get(0).to_rust_string_lossy(scope);
    if path.is_empty() {
        throw_error(scope, "EINVAL: napi load requires absolute path");
        return;
    }
    let resolver = {
        let handle = from_isolate(scope);
        let cjs = handle.0.borrow().cjs.clone();
        match cjs {
            Some(r) => r,
            None => {
                throw_error(scope, "cjs resolver not configured");
                return;
            }
        }
    };
    let abs = std::path::PathBuf::from(&path);
    if !resolver.is_path_admitted(abs.as_path()) {
        throw_error(scope, &format!("EACCES: load denied for '{path}'"));
        return;
    }
    let context = scope.get_current_context();
    match crate::napi::load_native_module(scope, context, abs.as_path()) {
        Ok(exports) => rv.set(exports),
        Err(err) => throw_error(scope, &err.to_string()),
    }
}

// ──────────────────────────────────────────────────────────────────────
// Helpers for byte-array returns
// ──────────────────────────────────────────────────────────────────────

fn bytes_to_uint8array<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    bytes: &[u8],
) -> v8::Local<'s, v8::Uint8Array> {
    let store = v8::ArrayBuffer::new_backing_store_from_vec(bytes.to_vec()).make_shared();
    let ab = v8::ArrayBuffer::with_backing_store(scope, &store);
    v8::Uint8Array::new(scope, ab, 0, bytes.len()).expect("uint8array")
}

fn string_arg<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: &v8::FunctionCallbackArguments<'s>,
    idx: i32,
) -> String {
    args.get(idx).to_rust_string_lossy(scope)
}

fn bool_arg<'s>(args: &v8::FunctionCallbackArguments<'s>, idx: i32) -> bool {
    args.get(idx).is_true()
}

fn bytes_arg<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: &v8::FunctionCallbackArguments<'s>,
    idx: i32,
) -> Option<Vec<u8>> {
    let v = args.get(idx);
    read_bytes_arg(scope, v).map(|b| b.to_vec())
}

// ──────────────────────────────────────────────────────────────────────
// node:os
// ──────────────────────────────────────────────────────────────────────

fn op_os_arch<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let s = v8::String::new(scope, std::env::consts::ARCH).unwrap();
    rv.set(s.into());
}

fn op_os_platform<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let plat = match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    };
    let s = v8::String::new(scope, plat).unwrap();
    rv.set(s.into());
}

fn op_os_type<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let t = match std::env::consts::OS {
        "macos" => "Darwin",
        "linux" => "Linux",
        "windows" => "Windows_NT",
        other => other,
    };
    let s = v8::String::new(scope, t).unwrap();
    rv.set(s.into());
}

fn op_os_release<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let s = v8::String::new(scope, "0.0.0").unwrap();
    rv.set(s.into());
}

fn op_os_hostname<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let host = std::env::var("HOSTNAME").unwrap_or_else(|_| "localhost".to_owned());
    let s = v8::String::new(scope, &host).unwrap();
    rv.set(s.into());
}

fn op_os_tmpdir<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let dir = std::env::temp_dir().to_string_lossy().into_owned();
    let s = v8::String::new(scope, &dir).unwrap();
    rv.set(s.into());
}

fn op_os_homedir<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_default();
    let s = v8::String::new(scope, &home).unwrap();
    rv.set(s.into());
}

fn op_os_endianness<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let e = if cfg!(target_endian = "little") {
        "LE"
    } else {
        "BE"
    };
    let s = v8::String::new(scope, e).unwrap();
    rv.set(s.into());
}

fn op_os_uptime_secs<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    rv.set(v8::Number::new(scope, 0.0).into());
}

fn op_os_freemem<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    rv.set(v8::Number::new(scope, 0.0).into());
}

fn op_os_totalmem<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    rv.set(v8::Number::new(scope, 0.0).into());
}

fn op_os_cpus_count<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let n = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1);
    rv.set(v8::Number::new(scope, n as f64).into());
}

// ──────────────────────────────────────────────────────────────────────
// node:fs (sync subset)
// ──────────────────────────────────────────────────────────────────────

fn fs_handle_for(scope: &v8::Isolate) -> Option<crate::ops::FsHandle> {
    from_isolate(scope).0.borrow().fs.clone()
}

fn throw_fs_error(scope: &mut v8::PinScope<'_, '_>, err: &crate::ops::FsError) {
    throw_error(scope, &format!("{}: {}", err.code, err.message));
}

/// Maps a [`std::io::Error`] to a Node-style error `code` (`ENOENT`,
/// `EACCES`, …). Used by the fallback fs ops where the [`FsHandle`]
/// is absent and we delegate directly to `std::fs`. Mirrors the same
/// table as [`crate::ops::FsError::from_io`] so error codes look the
/// same regardless of whether the sandbox is wired up.
fn io_error_code(err: &std::io::Error) -> &'static str {
    use std::io::ErrorKind as K;
    match err.kind() {
        K::NotFound => "ENOENT",
        K::PermissionDenied => "EACCES",
        K::AlreadyExists => "EEXIST",
        K::InvalidInput | K::InvalidData => "EINVAL",
        K::Unsupported => "ENOSYS",
        _ => "EIO",
    }
}

fn map_io_err(err: std::io::Error) -> (&'static str, String) {
    let code = io_error_code(&err);
    (code, err.to_string())
}

fn op_fs_read<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let path = string_arg(scope, &args, 0);
    let result = if let Some(fs) = fs_handle_for(scope) {
        fs.read(&path).map_err(|e| (e.code, e.message))
    } else {
        std::fs::read(&path).map_err(map_io_err)
    };
    match result {
        Ok(bytes) => {
            let arr = bytes_to_uint8array(scope, &bytes);
            rv.set(arr.into());
        }
        Err((code, msg)) => throw_error(scope, &format!("{code}: {msg}")),
    }
}

fn op_fs_write<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue<'s, v8::Value>,
) {
    let path = string_arg(scope, &args, 0);
    let Some(data) = bytes_arg(scope, &args, 1) else {
        throw_error(scope, "fs.write: data must be Uint8Array");
        return;
    };
    let result = if let Some(fs) = fs_handle_for(scope) {
        fs.write(&path, &data).map_err(|e| (e.code, e.message))
    } else {
        std::fs::write(&path, &data).map_err(map_io_err)
    };
    if let Err((code, msg)) = result {
        throw_error(scope, &format!("{code}: {msg}"));
    }
}

fn op_fs_exists<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let path = string_arg(scope, &args, 0);
    let exists = fs_handle_for(scope).map_or_else(
        || std::path::Path::new(&path).exists(),
        |fs| fs.exists(&path),
    );
    rv.set(v8::Boolean::new(scope, exists).into());
}

fn op_fs_stat<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let path = string_arg(scope, &args, 0);
    let follow = bool_arg(&args, 1);
    let (size, is_file, is_dir, is_symlink, mtime_ms, mode) = if let Some(fs) = fs_handle_for(scope)
    {
        match fs.stat(&path, follow) {
            Ok(s) => (
                s.size as f64,
                s.is_file,
                s.is_dir,
                s.is_symlink,
                s.mtime_ms,
                f64::from(s.mode),
            ),
            Err(err) => {
                throw_fs_error(scope, &err);
                return;
            }
        }
    } else {
        let meta = if follow {
            std::fs::metadata(&path)
        } else {
            std::fs::symlink_metadata(&path)
        };
        let meta = match meta {
            Ok(m) => m,
            Err(err) => {
                throw_error(scope, &format!("ENOENT: {err}"));
                return;
            }
        };
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0.0, |d| d.as_secs_f64() * 1000.0);
        (
            meta.len() as f64,
            meta.is_file(),
            meta.is_dir(),
            meta.file_type().is_symlink(),
            mtime,
            420.0,
        )
    };
    let obj = v8::Object::new(scope);
    let size_v = v8::Number::new(scope, size);
    let mtime_v = v8::Number::new(scope, mtime_ms);
    let mode_v = v8::Number::new(scope, mode);
    let f = v8::Boolean::new(scope, is_file);
    let d = v8::Boolean::new(scope, is_dir);
    let l = v8::Boolean::new(scope, is_symlink);
    set_property(scope, obj, "size", size_v.into());
    set_property(scope, obj, "mtime_ms", mtime_v.into());
    set_property(scope, obj, "atime_ms", mtime_v.into());
    set_property(scope, obj, "ctime_ms", mtime_v.into());
    set_property(scope, obj, "birthtime_ms", mtime_v.into());
    set_property(scope, obj, "mode", mode_v.into());
    set_property(scope, obj, "is_file", f.into());
    set_property(scope, obj, "is_dir", d.into());
    set_property(scope, obj, "is_symlink", l.into());
    rv.set(obj.into());
}

fn op_fs_realpath<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let path = string_arg(scope, &args, 0);
    let result = if let Some(fs) = fs_handle_for(scope) {
        fs.realpath(&path).map_err(|e| (e.code, e.message))
    } else {
        std::fs::canonicalize(&path).map_err(map_io_err)
    };
    match result {
        Ok(p) => {
            let s = v8::String::new(scope, &p.to_string_lossy()).unwrap();
            rv.set(s.into());
        }
        Err((code, msg)) => throw_error(scope, &format!("{code}: {msg}")),
    }
}

fn op_fs_readdir<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let path = string_arg(scope, &args, 0);
    let collected: Vec<(String, bool, bool)> = if let Some(fs) = fs_handle_for(scope) {
        match fs.read_dir(&path) {
            Ok(entries) => entries
                .into_iter()
                .map(|e| (e.name, e.is_dir, e.is_symlink))
                .collect(),
            Err(err) => {
                throw_fs_error(scope, &err);
                return;
            }
        }
    } else {
        let entries = match std::fs::read_dir(&path) {
            Ok(it) => it,
            Err(err) => {
                throw_error(scope, &format!("ENOENT: {err}"));
                return;
            }
        };
        entries
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                let ft = e.file_type().ok()?;
                Some((name, ft.is_dir(), ft.is_symlink()))
            })
            .collect()
    };
    let arr = v8::Array::new(scope, i32::try_from(collected.len()).unwrap_or(0));
    for (i, (name, is_dir, is_link)) in collected.iter().enumerate() {
        let obj = v8::Object::new(scope);
        let n = v8::String::new(scope, name).unwrap();
        let d = v8::Boolean::new(scope, *is_dir);
        let l = v8::Boolean::new(scope, *is_link);
        set_property(scope, obj, "name", n.into());
        set_property(scope, obj, "is_dir", d.into());
        set_property(scope, obj, "is_symlink", l.into());
        arr.set_index(scope, u32::try_from(i).unwrap_or(0), obj.into());
    }
    rv.set(arr.into());
}

fn op_fs_mkdir<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue<'s, v8::Value>,
) {
    let path = string_arg(scope, &args, 0);
    let recursive = bool_arg(&args, 1);
    let result = if let Some(fs) = fs_handle_for(scope) {
        fs.mkdir(&path, recursive).map_err(|e| (e.code, e.message))
    } else if recursive {
        std::fs::create_dir_all(&path).map_err(map_io_err)
    } else {
        std::fs::create_dir(&path).map_err(map_io_err)
    };
    if let Err((code, msg)) = result {
        throw_error(scope, &format!("{code}: {msg}"));
    }
}

fn op_fs_rm<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue<'s, v8::Value>,
) {
    let path = string_arg(scope, &args, 0);
    let recursive = bool_arg(&args, 1);
    let result = if let Some(fs) = fs_handle_for(scope) {
        fs.remove(&path, recursive).map_err(|e| (e.code, e.message))
    } else {
        let p = std::path::Path::new(&path);
        if !p.exists() {
            Ok(())
        } else if p.is_dir() {
            if recursive {
                std::fs::remove_dir_all(p)
            } else {
                std::fs::remove_dir(p)
            }
            .map_err(map_io_err)
        } else {
            std::fs::remove_file(p).map_err(map_io_err)
        }
    };
    if let Err((code, msg)) = result {
        throw_error(scope, &format!("{code}: {msg}"));
    }
}

fn op_fs_copy<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue<'s, v8::Value>,
) {
    let src = string_arg(scope, &args, 0);
    let dst = string_arg(scope, &args, 1);
    let result = if let Some(fs) = fs_handle_for(scope) {
        fs.copy(&src, &dst).map_err(|e| (e.code, e.message))
    } else {
        std::fs::copy(&src, &dst).map(|_| ()).map_err(map_io_err)
    };
    if let Err((code, msg)) = result {
        throw_error(scope, &format!("{code}: {msg}"));
    }
}

fn op_fs_readlink<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let path = string_arg(scope, &args, 0);
    let result = if let Some(fs) = fs_handle_for(scope) {
        fs.read_link(&path).map_err(|e| (e.code, e.message))
    } else {
        std::fs::read_link(&path).map_err(map_io_err)
    };
    match result {
        Ok(p) => {
            let s = v8::String::new(scope, &p.to_string_lossy()).unwrap();
            rv.set(s.into());
        }
        Err((code, msg)) => throw_error(scope, &format!("{code}: {msg}")),
    }
}

// ──────────────────────────────────────────────────────────────────────
// node:crypto
// ──────────────────────────────────────────────────────────────────────

fn op_crypto_hash<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    use digest::Digest;
    let algo = string_arg(scope, &args, 0);
    let Some(input) = bytes_arg(scope, &args, 1) else {
        throw_error(scope, "crypto.hash: data must be Uint8Array");
        return;
    };
    let digest: Vec<u8> = match algo.as_str() {
        "sha1" => {
            let mut h = sha1::Sha1::new();
            h.update(&input);
            h.finalize().to_vec()
        }
        "sha256" => {
            let mut h = sha2::Sha256::new();
            h.update(&input);
            h.finalize().to_vec()
        }
        "sha384" => {
            let mut h = sha2::Sha384::new();
            h.update(&input);
            h.finalize().to_vec()
        }
        "sha512" => {
            let mut h = sha2::Sha512::new();
            h.update(&input);
            h.finalize().to_vec()
        }
        "md5" => {
            let mut h = md5::Md5::new();
            h.update(&input);
            h.finalize().to_vec()
        }
        other => {
            throw_error(scope, &format!("ENOSYS: hash {other}"));
            return;
        }
    };
    let arr = bytes_to_uint8array(scope, &digest);
    rv.set(arr.into());
}

fn op_crypto_hmac<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    use hmac::Mac;
    let algo = string_arg(scope, &args, 0);
    let Some(key) = bytes_arg(scope, &args, 1) else {
        throw_error(scope, "hmac: key must be Uint8Array");
        return;
    };
    let Some(input) = bytes_arg(scope, &args, 2) else {
        throw_error(scope, "hmac: data must be Uint8Array");
        return;
    };
    let digest: Vec<u8> = match algo.as_str() {
        "sha1" => {
            let mut m = hmac::Hmac::<sha1::Sha1>::new_from_slice(&key).expect("hmac key");
            m.update(&input);
            m.finalize().into_bytes().to_vec()
        }
        "sha256" => {
            let mut m = hmac::Hmac::<sha2::Sha256>::new_from_slice(&key).expect("hmac key");
            m.update(&input);
            m.finalize().into_bytes().to_vec()
        }
        "sha384" => {
            let mut m = hmac::Hmac::<sha2::Sha384>::new_from_slice(&key).expect("hmac key");
            m.update(&input);
            m.finalize().into_bytes().to_vec()
        }
        "sha512" => {
            let mut m = hmac::Hmac::<sha2::Sha512>::new_from_slice(&key).expect("hmac key");
            m.update(&input);
            m.finalize().into_bytes().to_vec()
        }
        other => {
            throw_error(scope, &format!("ENOSYS: hmac {other}"));
            return;
        }
    };
    let arr = bytes_to_uint8array(scope, &digest);
    rv.set(arr.into());
}

fn op_crypto_random_bytes<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    use rand::RngCore;
    let len = args.get(0).uint32_value(scope).unwrap_or(0) as usize;
    let mut buf = vec![0u8; len];
    rand::rng().fill_bytes(&mut buf);
    let arr = bytes_to_uint8array(scope, &buf);
    rv.set(arr.into());
}

fn op_crypto_random_uuid<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let s = format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15],
    );
    let v = v8::String::new(scope, &s).unwrap();
    rv.set(v.into());
}

fn op_crypto_timing_safe_equal<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Some(a) = bytes_arg(scope, &args, 0) else {
        rv.set(v8::Boolean::new(scope, false).into());
        return;
    };
    let Some(b) = bytes_arg(scope, &args, 1) else {
        rv.set(v8::Boolean::new(scope, false).into());
        return;
    };
    if a.len() != b.len() {
        rv.set(v8::Boolean::new(scope, false).into());
        return;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    rv.set(v8::Boolean::new(scope, diff == 0).into());
}

fn op_crypto_aes_gcm_seal<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    use aes_gcm::aead::{Aead, KeyInit, Payload};
    use aes_gcm::{Aes256Gcm, Key, Nonce};
    let Some(key_bytes) = bytes_arg(scope, &args, 0) else {
        throw_error(scope, "aes_gcm_seal: key");
        return;
    };
    let Some(iv_bytes) = bytes_arg(scope, &args, 1) else {
        throw_error(scope, "aes_gcm_seal: iv");
        return;
    };
    let Some(plaintext) = bytes_arg(scope, &args, 2) else {
        throw_error(scope, "aes_gcm_seal: pt");
        return;
    };
    let aad = bytes_arg(scope, &args, 3).unwrap_or_default();
    if key_bytes.len() != 32 || iv_bytes.len() != 12 {
        throw_error(scope, "aes_gcm_seal: key=32 iv=12");
        return;
    }
    let key = Key::<Aes256Gcm>::from_slice(&key_bytes);
    let cipher = Aes256Gcm::new(key);
    let nonce = Nonce::from_slice(&iv_bytes);
    let payload = Payload {
        msg: &plaintext,
        aad: &aad,
    };
    match cipher.encrypt(nonce, payload) {
        Ok(ct) => {
            let arr = bytes_to_uint8array(scope, &ct);
            rv.set(arr.into());
        }
        Err(_) => throw_error(scope, "aes_gcm_seal: encrypt failed"),
    }
}

fn op_crypto_aes_gcm_open<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    use aes_gcm::aead::{Aead, KeyInit, Payload};
    use aes_gcm::{Aes256Gcm, Key, Nonce};
    let Some(key_bytes) = bytes_arg(scope, &args, 0) else {
        throw_error(scope, "aes_gcm_open: key");
        return;
    };
    let Some(iv_bytes) = bytes_arg(scope, &args, 1) else {
        throw_error(scope, "aes_gcm_open: iv");
        return;
    };
    let Some(ct) = bytes_arg(scope, &args, 2) else {
        throw_error(scope, "aes_gcm_open: ct");
        return;
    };
    let aad = bytes_arg(scope, &args, 3).unwrap_or_default();
    if key_bytes.len() != 32 || iv_bytes.len() != 12 {
        throw_error(scope, "aes_gcm_open: key=32 iv=12");
        return;
    }
    let key = Key::<Aes256Gcm>::from_slice(&key_bytes);
    let cipher = Aes256Gcm::new(key);
    let nonce = Nonce::from_slice(&iv_bytes);
    let payload = Payload {
        msg: &ct,
        aad: &aad,
    };
    match cipher.decrypt(nonce, payload) {
        Ok(pt) => {
            let arr = bytes_to_uint8array(scope, &pt);
            rv.set(arr.into());
        }
        Err(_) => throw_error(scope, "aes_gcm_open: auth failed"),
    }
}

// ──────────────────────────────────────────────────────────────────────
// node:zlib
// ──────────────────────────────────────────────────────────────────────

fn op_zlib_encode<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    use std::io::Write;
    let algo = string_arg(scope, &args, 0);
    let Some(input) = bytes_arg(scope, &args, 1) else {
        throw_error(scope, "zlib.encode: data must be Uint8Array");
        return;
    };
    let out: std::io::Result<Vec<u8>> = match algo.as_str() {
        "gzip" => {
            let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            e.write_all(&input).and_then(|()| e.finish())
        }
        "deflate" => {
            let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            e.write_all(&input).and_then(|()| e.finish())
        }
        "deflate-raw" => {
            let mut e =
                flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
            e.write_all(&input).and_then(|()| e.finish())
        }
        "brotli" => {
            let mut buf = Vec::new();
            {
                let mut w = brotli::CompressorWriter::new(&mut buf, 4096, 4, 22);
                if let Err(err) = w.write_all(&input) {
                    throw_error(scope, &format!("EIO: {err}"));
                    return;
                }
            }
            Ok(buf)
        }
        other => {
            throw_error(scope, &format!("ENOSYS: zlib {other}"));
            return;
        }
    };
    match out {
        Ok(bytes) => {
            let arr = bytes_to_uint8array(scope, &bytes);
            rv.set(arr.into());
        }
        Err(err) => throw_error(scope, &format!("EIO: {err}")),
    }
}

fn op_zlib_decode<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    use std::io::Read;
    let algo = string_arg(scope, &args, 0);
    let Some(input) = bytes_arg(scope, &args, 1) else {
        throw_error(scope, "zlib.decode: data must be Uint8Array");
        return;
    };
    let mut out = Vec::new();
    let res: std::io::Result<()> = match algo.as_str() {
        "gzip" => flate2::read::GzDecoder::new(&input[..])
            .read_to_end(&mut out)
            .map(|_| ()),
        "deflate" => flate2::read::ZlibDecoder::new(&input[..])
            .read_to_end(&mut out)
            .map(|_| ()),
        "deflate-raw" => flate2::read::DeflateDecoder::new(&input[..])
            .read_to_end(&mut out)
            .map(|_| ()),
        "brotli" => brotli::Decompressor::new(&input[..], 4096)
            .read_to_end(&mut out)
            .map(|_| ()),
        other => {
            throw_error(scope, &format!("ENOSYS: zlib {other}"));
            return;
        }
    };
    match res {
        Ok(()) => {
            let arr = bytes_to_uint8array(scope, &out);
            rv.set(arr.into());
        }
        Err(err) => throw_error(scope, &format!("EIO: {err}")),
    }
}

// ──────────────────────────────────────────────────────────────────────
// node:dns ops
// ──────────────────────────────────────────────────────────────────────
//
// Each op follows the same shape:
//   1. Pull the call arguments off `args`, allocate a fresh
//      `PromiseResolver`, and clone the bridge's async-completion
//      sender.
//   2. `tokio::task::spawn_local` an async block that runs the
//      lookup off the JS critical path. On completion it builds a
//      `Settler` closure that marshals the result into `v8::Value`s
//      and forwards it through the channel.
//   3. The engine pump drains the channel on every tick and resolves
//      / rejects each promise on the isolate thread (where
//      `v8::Local` values are valid).

use std::future::Future;

use crate::ops::DnsError;

/// Schedules `work` off-isolate and resolves the returned promise
/// with the value built by `on_ok` (run on the isolate thread).
///
/// On `Err(DnsError)` the promise rejects with a Node-style `Error`
/// whose `.code` carries the mapped error string.
fn schedule_dns<'s, Fut, T, Mk>(
    scope: &mut v8::PinScope<'s, '_>,
    work: Fut,
    on_ok: Mk,
) -> Option<v8::Local<'s, v8::Promise>>
where
    Fut: Future<Output = Result<T, DnsError>> + 'static,
    T: 'static,
    Mk: for<'a, 'b> FnOnce(&mut v8::PinScope<'a, 'b>, T) -> v8::Local<'a, v8::Value> + 'static,
{
    let resolver = v8::PromiseResolver::new(scope)?;
    let promise = resolver.get_promise(scope);
    let global = v8::Global::new(scope, resolver);
    let handle = from_isolate(scope);
    let tx = handle.0.borrow().async_completions_tx.clone();

    tokio::task::spawn_local(async move {
        let result = work.await;
        let settler: super::async_ops::Settler = match result {
            Ok(value) => Box::new(move |scope, resolver| {
                let v = on_ok(scope, value);
                resolver.resolve(scope, v);
            }),
            Err(err) => super::async_ops::reject_with_code(err.message, err.code),
        };
        let _ = tx.send(super::async_ops::Completion::new(global, settler));
    });

    Some(promise)
}

fn op_dns_lookup<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let host = args.get(0).to_rust_string_lossy(scope);
    let family_node = args.get(1).uint32_value(scope).unwrap_or(0);
    let all = args.get(2).boolean_value(scope);
    let max = if all { usize::MAX } else { 1 };
    let family = crate::ops::LookupFamily::from_node(family_node);

    let work = async move { crate::ops::dns_lookup(&host, family, max).await };
    let promise = schedule_dns(scope, work, move |scope, results| {
        if all {
            let arr = v8::Array::new(scope, results.len() as i32);
            for (i, r) in results.iter().enumerate() {
                let obj = make_lookup_obj(scope, r);
                arr.set_index(scope, i as u32, obj.into());
            }
            arr.into()
        } else {
            let first = results.into_iter().next().expect("at least one result");
            make_lookup_obj(scope, &first).into()
        }
    });
    if let Some(p) = promise {
        rv.set(p.into());
    }
}

fn make_lookup_obj<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    result: &crate::ops::LookupResult,
) -> v8::Local<'s, v8::Object> {
    let obj = v8::Object::new(scope);
    set_string_field(scope, obj, "address", &result.address.to_string());
    let fam_key = v8::String::new(scope, "family").unwrap();
    let fam_val = v8::Number::new(scope, f64::from(result.family));
    obj.set(scope, fam_key.into(), fam_val.into());
    obj
}

fn op_dns_resolve4<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let host = args.get(0).to_rust_string_lossy(scope);
    let work = async move { crate::ops::dns_resolve4(&host).await };
    let promise = schedule_dns(scope, work, |scope, ips| ip_array(scope, &ips).into());
    if let Some(p) = promise {
        rv.set(p.into());
    }
}

fn op_dns_resolve6<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let host = args.get(0).to_rust_string_lossy(scope);
    let work = async move { crate::ops::dns_resolve6(&host).await };
    let promise = schedule_dns(scope, work, |scope, ips| ip_array(scope, &ips).into());
    if let Some(p) = promise {
        rv.set(p.into());
    }
}

fn ip_array<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    ips: &[std::net::IpAddr],
) -> v8::Local<'s, v8::Array> {
    let arr = v8::Array::new(scope, ips.len() as i32);
    for (i, ip) in ips.iter().enumerate() {
        let s = v8::String::new(scope, &ip.to_string()).unwrap();
        arr.set_index(scope, i as u32, s.into());
    }
    arr
}

fn op_dns_resolve_mx<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let host = args.get(0).to_rust_string_lossy(scope);
    let work = async move { crate::ops::dns_resolve_mx(&host).await };
    let promise = schedule_dns(scope, work, |scope, records| {
        let arr = v8::Array::new(scope, records.len() as i32);
        for (i, r) in records.iter().enumerate() {
            let obj = v8::Object::new(scope);
            let prio_key = v8::String::new(scope, "priority").unwrap();
            let prio_val = v8::Number::new(scope, f64::from(r.priority));
            obj.set(scope, prio_key.into(), prio_val.into());
            set_string_field(scope, obj, "exchange", &r.exchange);
            arr.set_index(scope, i as u32, obj.into());
        }
        arr.into()
    });
    if let Some(p) = promise {
        rv.set(p.into());
    }
}

fn op_dns_resolve_txt<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let host = args.get(0).to_rust_string_lossy(scope);
    let work = async move { crate::ops::dns_resolve_txt(&host).await };
    let promise = schedule_dns(scope, work, |scope, records| {
        let arr = v8::Array::new(scope, records.len() as i32);
        for (i, chunks) in records.iter().enumerate() {
            let inner = v8::Array::new(scope, chunks.len() as i32);
            for (j, chunk) in chunks.iter().enumerate() {
                let s = v8::String::new(scope, chunk).unwrap();
                inner.set_index(scope, j as u32, s.into());
            }
            arr.set_index(scope, i as u32, inner.into());
        }
        arr.into()
    });
    if let Some(p) = promise {
        rv.set(p.into());
    }
}

fn op_dns_resolve_cname<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let host = args.get(0).to_rust_string_lossy(scope);
    let work = async move { crate::ops::dns_resolve_cname(&host).await };
    let promise = schedule_dns(scope, work, |scope, names| {
        string_array(scope, &names).into()
    });
    if let Some(p) = promise {
        rv.set(p.into());
    }
}

fn op_dns_resolve_ns<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let host = args.get(0).to_rust_string_lossy(scope);
    let work = async move { crate::ops::dns_resolve_ns(&host).await };
    let promise = schedule_dns(scope, work, |scope, names| {
        string_array(scope, &names).into()
    });
    if let Some(p) = promise {
        rv.set(p.into());
    }
}

fn string_array<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    items: &[String],
) -> v8::Local<'s, v8::Array> {
    let arr = v8::Array::new(scope, items.len() as i32);
    for (i, item) in items.iter().enumerate() {
        let s = v8::String::new(scope, item).unwrap();
        arr.set_index(scope, i as u32, s.into());
    }
    arr
}

fn op_dns_resolve_srv<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let host = args.get(0).to_rust_string_lossy(scope);
    let work = async move { crate::ops::dns_resolve_srv(&host).await };
    let promise = schedule_dns(scope, work, |scope, records| {
        let arr = v8::Array::new(scope, records.len() as i32);
        for (i, r) in records.iter().enumerate() {
            let obj = v8::Object::new(scope);
            let prio_key = v8::String::new(scope, "priority").unwrap();
            let prio_val = v8::Number::new(scope, f64::from(r.priority));
            obj.set(scope, prio_key.into(), prio_val.into());
            let weight_key = v8::String::new(scope, "weight").unwrap();
            let weight_val = v8::Number::new(scope, f64::from(r.weight));
            obj.set(scope, weight_key.into(), weight_val.into());
            let port_key = v8::String::new(scope, "port").unwrap();
            let port_val = v8::Number::new(scope, f64::from(r.port));
            obj.set(scope, port_key.into(), port_val.into());
            set_string_field(scope, obj, "name", &r.name);
            arr.set_index(scope, i as u32, obj.into());
        }
        arr.into()
    });
    if let Some(p) = promise {
        rv.set(p.into());
    }
}

fn op_dns_reverse<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let ip_str = args.get(0).to_rust_string_lossy(scope);
    let parsed: Result<std::net::IpAddr, _> = ip_str.parse();
    match parsed {
        Ok(ip) => {
            let work = async move { crate::ops::dns_reverse(ip).await };
            let promise = schedule_dns(scope, work, |scope, names| {
                string_array(scope, &names).into()
            });
            if let Some(p) = promise {
                rv.set(p.into());
            }
        }
        Err(_) => {
            throw_type_error(scope, "dns.reverse: invalid IP address");
        }
    }
}

/// Sleeps for `ms` milliseconds and resolves the returned promise.
///
/// Backed by `tokio::time::sleep` and the shared async-completion
/// channel, so the JS pump drives it on the isolate thread once the
/// timer fires. `ms` is clamped to `[0, i32::MAX]` so a misbehaving
/// caller cannot register a future that never fires.
fn op_timer_sleep<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let ms_raw = args.get(0).number_value(scope).unwrap_or(0.0);
    let ms = if ms_raw.is_finite() && ms_raw > 0.0 {
        ms_raw.min(f64::from(i32::MAX)) as u64
    } else {
        0
    };

    let Some(resolver) = v8::PromiseResolver::new(scope) else {
        rv.set_undefined();
        return;
    };
    let promise = resolver.get_promise(scope);
    let global = v8::Global::new(scope, resolver);
    let handle = from_isolate(scope);
    let tx = handle.0.borrow().async_completions_tx.clone();

    tokio::task::spawn_local(async move {
        tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
        let settler: super::async_ops::Settler = Box::new(|scope, resolver| {
            let undef = v8::undefined(scope);
            resolver.resolve(scope, undef.into());
        });
        let _ = tx.send(super::async_ops::Completion::new(global, settler));
    });

    rv.set(promise.into());
}

// ──────────────────────────────────────────────────────────────────────
// node:net ops
// ──────────────────────────────────────────────────────────────────────

use crate::ops::{AddressInfo, NetError};

const NET_BRIDGE_TARGET: &str = "nexide::engine::bridge::net";

fn make_address_obj<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    info: &AddressInfo,
) -> v8::Local<'s, v8::Object> {
    let obj = v8::Object::new(scope);
    set_string_field(scope, obj, "address", &info.address);
    let port_key = v8::String::new(scope, "port").unwrap();
    let port_val = v8::Number::new(scope, f64::from(info.port));
    obj.set(scope, port_key.into(), port_val.into());
    let fam_key = v8::String::new(scope, "family").unwrap();
    let fam_val = v8::Number::new(scope, f64::from(info.family));
    obj.set(scope, fam_key.into(), fam_val.into());
    obj
}

fn reject_net<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    resolver: v8::Local<'s, v8::PromiseResolver>,
    err: &NetError,
) {
    let msg = v8::String::new(scope, &err.message).unwrap_or_else(|| v8::String::empty(scope));
    let exc = v8::Exception::error(scope, msg);
    if let Ok(obj) = TryInto::<v8::Local<v8::Object>>::try_into(exc) {
        set_string_field(scope, obj, "code", err.code);
    }
    resolver.reject(scope, exc);
}

fn net_settler_err(err: NetError) -> super::async_ops::Settler {
    Box::new(move |scope, resolver| reject_net(scope, resolver, &err))
}

fn op_net_connect<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let host = args.get(0).to_rust_string_lossy(scope);
    let port = args.get(1).uint32_value(scope).unwrap_or(0) as u16;

    let Some(resolver) = v8::PromiseResolver::new(scope) else {
        rv.set_undefined();
        return;
    };
    let promise = resolver.get_promise(scope);
    let global = v8::Global::new(scope, resolver);
    let handle = from_isolate(scope);
    let tx = handle.0.borrow().async_completions_tx.clone();
    let table = handle.0.borrow().net_streams.clone();

    tokio::task::spawn_local(async move {
        let result = crate::ops::net_connect(&host, port).await;
        let settler: super::async_ops::Settler = match result {
            Ok((stream, local, remote)) => {
                let slot = std::rc::Rc::new(stream);
                let id = table.insert(slot);
                tracing::debug!(
                    target: NET_BRIDGE_TARGET,
                    stream_id = id,
                    local = %local,
                    remote = %remote,
                    "net stream slot allocated",
                );
                Box::new(move |scope, resolver| {
                    let obj = v8::Object::new(scope);
                    let id_key = v8::String::new(scope, "id").unwrap();
                    let id_val = v8::Number::new(scope, f64::from(id));
                    obj.set(scope, id_key.into(), id_val.into());
                    let local_obj = make_address_obj(scope, &local);
                    let local_key = v8::String::new(scope, "local").unwrap();
                    obj.set(scope, local_key.into(), local_obj.into());
                    let remote_obj = make_address_obj(scope, &remote);
                    let remote_key = v8::String::new(scope, "remote").unwrap();
                    obj.set(scope, remote_key.into(), remote_obj.into());
                    resolver.resolve(scope, obj.into());
                })
            }
            Err(err) => net_settler_err(err),
        };
        let _ = tx.send(super::async_ops::Completion::new(global, settler));
    });
    rv.set(promise.into());
}

fn op_net_listen<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let host = args.get(0).to_rust_string_lossy(scope);
    let port = args.get(1).uint32_value(scope).unwrap_or(0) as u16;
    let Some(resolver) = v8::PromiseResolver::new(scope) else {
        rv.set_undefined();
        return;
    };
    let promise = resolver.get_promise(scope);
    let global = v8::Global::new(scope, resolver);
    let handle = from_isolate(scope);
    let tx = handle.0.borrow().async_completions_tx.clone();
    let table = handle.0.borrow().net_listeners.clone();

    tokio::task::spawn_local(async move {
        let settler: super::async_ops::Settler = match crate::ops::net_listen(&host, port).await {
            Ok((listener, addr)) => {
                let id = table.insert(std::rc::Rc::new(listener));
                Box::new(move |scope, resolver| {
                    let obj = v8::Object::new(scope);
                    let id_key = v8::String::new(scope, "id").unwrap();
                    let id_val = v8::Number::new(scope, f64::from(id));
                    obj.set(scope, id_key.into(), id_val.into());
                    let addr_obj = make_address_obj(scope, &addr);
                    let addr_key = v8::String::new(scope, "address").unwrap();
                    obj.set(scope, addr_key.into(), addr_obj.into());
                    resolver.resolve(scope, obj.into());
                })
            }
            Err(err) => net_settler_err(err),
        };
        let _ = tx.send(super::async_ops::Completion::new(global, settler));
    });
    rv.set(promise.into());
}

fn op_net_accept<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let listener_id = args.get(0).uint32_value(scope).unwrap_or(0);
    let Some(resolver) = v8::PromiseResolver::new(scope) else {
        rv.set_undefined();
        return;
    };
    let promise = resolver.get_promise(scope);
    let global = v8::Global::new(scope, resolver);
    let handle = from_isolate(scope);
    let tx = handle.0.borrow().async_completions_tx.clone();
    let listeners = handle.0.borrow().net_listeners.clone();
    let streams = handle.0.borrow().net_streams.clone();

    let Some(listener) = listeners.with(listener_id, std::rc::Rc::clone) else {
        let err = NetError::new("EBADF", "listener has been closed");
        reject_net(scope, v8::Local::new(scope, &global), &err);
        rv.set(promise.into());
        return;
    };

    tokio::task::spawn_local(async move {
        let settler: super::async_ops::Settler = match crate::ops::net_accept(&listener).await {
            Ok((stream, local, remote)) => {
                let slot = std::rc::Rc::new(stream);
                let id = streams.insert(slot);
                Box::new(move |scope, resolver| {
                    let obj = v8::Object::new(scope);
                    let id_key = v8::String::new(scope, "id").unwrap();
                    let id_val = v8::Number::new(scope, f64::from(id));
                    obj.set(scope, id_key.into(), id_val.into());
                    let local_obj = make_address_obj(scope, &local);
                    let local_key = v8::String::new(scope, "local").unwrap();
                    obj.set(scope, local_key.into(), local_obj.into());
                    let remote_obj = make_address_obj(scope, &remote);
                    let remote_key = v8::String::new(scope, "remote").unwrap();
                    obj.set(scope, remote_key.into(), remote_obj.into());
                    resolver.resolve(scope, obj.into());
                })
            }
            Err(err) => net_settler_err(err),
        };
        let _ = tx.send(super::async_ops::Completion::new(global, settler));
    });
    rv.set(promise.into());
}

fn op_net_read<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let stream_id = args.get(0).uint32_value(scope).unwrap_or(0);
    let max = args.get(1).uint32_value(scope).unwrap_or(64 * 1024) as usize;
    let Some(resolver) = v8::PromiseResolver::new(scope) else {
        rv.set_undefined();
        return;
    };
    let promise = resolver.get_promise(scope);
    let global = v8::Global::new(scope, resolver);
    let handle = from_isolate(scope);
    let tx = handle.0.borrow().async_completions_tx.clone();
    let streams = handle.0.borrow().net_streams.clone();

    let Some(slot) = streams.with(stream_id, std::rc::Rc::clone) else {
        tracing::warn!(
            target: NET_BRIDGE_TARGET,
            stream_id,
            op = "read",
            "EBADF: read on closed slot",
        );
        let err = NetError::new("EBADF", "socket has been closed");
        reject_net(scope, v8::Local::new(scope, &global), &err);
        rv.set(promise.into());
        return;
    };

    tokio::task::spawn_local(async move {
        let result = crate::ops::net_read_chunk(&slot, max).await;
        let settler: super::async_ops::Settler = match result {
            Ok(bytes) => Box::new(move |scope, resolver| {
                let store = v8::ArrayBuffer::new_backing_store_from_vec(bytes).make_shared();
                let buf = v8::ArrayBuffer::with_backing_store(scope, &store);
                let view = v8::Uint8Array::new(scope, buf, 0, buf.byte_length()).unwrap();
                resolver.resolve(scope, view.into());
            }),
            Err(err) => net_settler_err(err),
        };
        let _ = tx.send(super::async_ops::Completion::new(global, settler));
    });
    rv.set(promise.into());
}

fn op_net_write<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let stream_id = args.get(0).uint32_value(scope).unwrap_or(0);
    let data = match read_uint8_array(scope, args.get(1)) {
        Some(bytes) => bytes,
        None => {
            throw_type_error(scope, "net.write: expected Uint8Array");
            return;
        }
    };
    let Some(resolver) = v8::PromiseResolver::new(scope) else {
        rv.set_undefined();
        return;
    };
    let promise = resolver.get_promise(scope);
    let global = v8::Global::new(scope, resolver);
    let handle = from_isolate(scope);
    let tx = handle.0.borrow().async_completions_tx.clone();
    let streams = handle.0.borrow().net_streams.clone();

    let Some(slot) = streams.with(stream_id, std::rc::Rc::clone) else {
        tracing::warn!(
            target: NET_BRIDGE_TARGET,
            stream_id,
            op = "write",
            len = data.len(),
            "EBADF: write on closed slot",
        );
        let err = NetError::new("EBADF", "socket has been closed");
        reject_net(scope, v8::Local::new(scope, &global), &err);
        rv.set(promise.into());
        return;
    };

    tokio::task::spawn_local(async move {
        let result = crate::ops::net_write_all(&slot, &data).await;
        let settler: super::async_ops::Settler = match result {
            Ok(()) => Box::new(move |scope, resolver| {
                let undef = v8::undefined(scope);
                resolver.resolve(scope, undef.into());
            }),
            Err(err) => net_settler_err(err),
        };
        let _ = tx.send(super::async_ops::Completion::new(global, settler));
    });
    rv.set(promise.into());
}

fn op_net_close_stream<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let stream_id = args.get(0).uint32_value(scope).unwrap_or(0);
    let handle = from_isolate(scope);
    let removed = handle.0.borrow().net_streams.remove(stream_id);
    if removed {
        tracing::debug!(target: NET_BRIDGE_TARGET, stream_id, "net stream slot released");
    } else {
        tracing::trace!(
            target: NET_BRIDGE_TARGET,
            stream_id,
            "net stream close on already-closed slot",
        );
    }
    let result = v8::Boolean::new(scope, removed);
    rv.set(result.into());
}

fn op_net_close_listener<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let listener_id = args.get(0).uint32_value(scope).unwrap_or(0);
    let handle = from_isolate(scope);
    let removed = handle.0.borrow().net_listeners.remove(listener_id);
    if removed {
        tracing::debug!(
            target: NET_BRIDGE_TARGET,
            listener_id,
            "net listener slot released",
        );
    }
    let result = v8::Boolean::new(scope, removed);
    rv.set(result.into());
}

fn op_net_set_nodelay<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let stream_id = args.get(0).uint32_value(scope).unwrap_or(0);
    let enable = args.get(1).boolean_value(scope);
    let handle = from_isolate(scope);
    let streams = handle.0.borrow().net_streams.clone();
    let applied = streams
        .with(stream_id, |slot| slot.set_nodelay(enable).is_ok())
        .unwrap_or(false);
    let result = v8::Boolean::new(scope, applied);
    rv.set(result.into());
}

fn op_net_set_keepalive<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue<'s, v8::Value>,
) {
    let _stream_id = args.get(0).uint32_value(scope).unwrap_or(0);
    let _enable = args.get(1).boolean_value(scope);
    let _ = scope;
}

fn read_uint8_array<'s>(
    _scope: &mut v8::PinScope<'s, '_>,
    value: v8::Local<'s, v8::Value>,
) -> Option<Vec<u8>> {
    let view = v8::Local::<v8::Uint8Array>::try_from(value).ok()?;
    let len = view.byte_length();
    let mut out = vec![0u8; len];
    let copied = view.copy_contents(&mut out);
    if copied != len {
        return None;
    }
    Some(out)
}

// ──────────────────────────────────────────────────────────────────────
// node:tls ops
// ──────────────────────────────────────────────────────────────────────

fn op_tls_connect<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let host = args.get(0).to_rust_string_lossy(scope);
    let port = args.get(1).uint32_value(scope).unwrap_or(0) as u16;
    let Some(resolver) = v8::PromiseResolver::new(scope) else {
        rv.set_undefined();
        return;
    };
    let promise = resolver.get_promise(scope);
    let global = v8::Global::new(scope, resolver);
    let handle = from_isolate(scope);
    let tx = handle.0.borrow().async_completions_tx.clone();
    let table = handle.0.borrow().tls_streams.clone();

    tokio::task::spawn_local(async move {
        let settler: super::async_ops::Settler = match crate::ops::tls_connect(&host, port).await {
            Ok((stream, local, remote)) => {
                let id = table.insert(std::rc::Rc::new(tokio::sync::Mutex::new(stream)));
                Box::new(move |scope, resolver| {
                    let obj = v8::Object::new(scope);
                    let id_key = v8::String::new(scope, "id").unwrap();
                    let id_val = v8::Number::new(scope, f64::from(id));
                    obj.set(scope, id_key.into(), id_val.into());
                    let local_obj = make_address_obj(scope, &local);
                    let local_key = v8::String::new(scope, "local").unwrap();
                    obj.set(scope, local_key.into(), local_obj.into());
                    let remote_obj = make_address_obj(scope, &remote);
                    let remote_key = v8::String::new(scope, "remote").unwrap();
                    obj.set(scope, remote_key.into(), remote_obj.into());
                    resolver.resolve(scope, obj.into());
                })
            }
            Err(err) => net_settler_err(err),
        };
        let _ = tx.send(super::async_ops::Completion::new(global, settler));
    });
    rv.set(promise.into());
}

/// Upgrades an existing `op_net_connect` socket (identified by its
/// JS-side handle id) to TLS, performing a client handshake on top
/// of the live TCP stream. Mirrors `tls.connect({ socket })` semantics
/// from `node:tls`, which protocols like PostgreSQL `SSLRequest`,
/// SMTP `STARTTLS` and IMAP/POP3 `STARTTLS` rely on. Removes the
/// entry from `net_streams` on success; the JS Socket handle becomes
/// invalid and must not be used afterwards.
fn op_tls_upgrade<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let net_id = args.get(0).uint32_value(scope).unwrap_or(0);
    let host = args.get(1).to_rust_string_lossy(scope);
    let Some(resolver) = v8::PromiseResolver::new(scope) else {
        rv.set_undefined();
        return;
    };
    let promise = resolver.get_promise(scope);
    let global = v8::Global::new(scope, resolver);
    let handle = from_isolate(scope);
    let tx = handle.0.borrow().async_completions_tx.clone();
    let net_streams = handle.0.borrow().net_streams.clone();
    let tls_streams = handle.0.borrow().tls_streams.clone();

    let Some(slot) = net_streams.take(net_id) else {
        tracing::warn!(
            target: NET_BRIDGE_TARGET,
            stream_id = net_id,
            op = "tls_upgrade",
            "EBADF: tls_upgrade on closed net slot",
        );
        let err = NetError::new("EBADF", "net stream has been closed");
        reject_net(scope, v8::Local::new(scope, &global), &err);
        rv.set(promise.into());
        return;
    };

    tokio::task::spawn_local(async move {
        let stream = match std::rc::Rc::try_unwrap(slot) {
            Ok(s) => s,
            Err(_rc) => {
                tracing::warn!(
                    target: NET_BRIDGE_TARGET,
                    stream_id = net_id,
                    op = "tls_upgrade",
                    "EBUSY: outstanding I/O on net slot; cannot upgrade",
                );
                let err = NetError::new(
                    "EBUSY",
                    "net stream still has outstanding I/O; cannot upgrade to TLS",
                );
                let settler = net_settler_err(err);
                let _ = tx.send(super::async_ops::Completion::new(global, settler));
                return;
            }
        };
        let result = crate::ops::tls_upgrade(stream, &host).await;
        let settler: super::async_ops::Settler = match result {
            Ok((tls, local, remote)) => {
                let id = tls_streams.insert(std::rc::Rc::new(tokio::sync::Mutex::new(tls)));
                tracing::debug!(
                    target: NET_BRIDGE_TARGET,
                    tls_id = id,
                    from_net_id = net_id,
                    local = %local,
                    remote = %remote,
                    "tls stream slot allocated from upgraded net stream",
                );
                Box::new(move |scope, resolver| {
                    let obj = v8::Object::new(scope);
                    let id_key = v8::String::new(scope, "id").unwrap();
                    let id_val = v8::Number::new(scope, f64::from(id));
                    obj.set(scope, id_key.into(), id_val.into());
                    let local_obj = make_address_obj(scope, &local);
                    let local_key = v8::String::new(scope, "local").unwrap();
                    obj.set(scope, local_key.into(), local_obj.into());
                    let remote_obj = make_address_obj(scope, &remote);
                    let remote_key = v8::String::new(scope, "remote").unwrap();
                    obj.set(scope, remote_key.into(), remote_obj.into());
                    resolver.resolve(scope, obj.into());
                })
            }
            Err(err) => net_settler_err(err),
        };
        let _ = tx.send(super::async_ops::Completion::new(global, settler));
    });
    rv.set(promise.into());
}

fn op_tls_read<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let stream_id = args.get(0).uint32_value(scope).unwrap_or(0);
    let max = args.get(1).uint32_value(scope).unwrap_or(64 * 1024) as usize;
    let Some(resolver) = v8::PromiseResolver::new(scope) else {
        rv.set_undefined();
        return;
    };
    let promise = resolver.get_promise(scope);
    let global = v8::Global::new(scope, resolver);
    let handle = from_isolate(scope);
    let tx = handle.0.borrow().async_completions_tx.clone();
    let streams = handle.0.borrow().tls_streams.clone();
    let Some(slot) = streams.with(stream_id, std::rc::Rc::clone) else {
        let err = NetError::new("EBADF", "tls stream has been closed");
        reject_net(scope, v8::Local::new(scope, &global), &err);
        rv.set(promise.into());
        return;
    };
    tokio::task::spawn_local(async move {
        let result = {
            let mut guard = slot.lock().await;
            crate::ops::tls_read_chunk(&mut guard, max).await
        };
        let settler: super::async_ops::Settler = match result {
            Ok(bytes) => Box::new(move |scope, resolver| {
                let store = v8::ArrayBuffer::new_backing_store_from_vec(bytes).make_shared();
                let buf = v8::ArrayBuffer::with_backing_store(scope, &store);
                let view = v8::Uint8Array::new(scope, buf, 0, buf.byte_length()).unwrap();
                resolver.resolve(scope, view.into());
            }),
            Err(err) => net_settler_err(err),
        };
        let _ = tx.send(super::async_ops::Completion::new(global, settler));
    });
    rv.set(promise.into());
}

fn op_tls_write<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let stream_id = args.get(0).uint32_value(scope).unwrap_or(0);
    let data = match read_uint8_array(scope, args.get(1)) {
        Some(b) => b,
        None => {
            throw_type_error(scope, "tls.write: expected Uint8Array");
            return;
        }
    };
    let Some(resolver) = v8::PromiseResolver::new(scope) else {
        rv.set_undefined();
        return;
    };
    let promise = resolver.get_promise(scope);
    let global = v8::Global::new(scope, resolver);
    let handle = from_isolate(scope);
    let tx = handle.0.borrow().async_completions_tx.clone();
    let streams = handle.0.borrow().tls_streams.clone();
    let Some(slot) = streams.with(stream_id, std::rc::Rc::clone) else {
        let err = NetError::new("EBADF", "tls stream has been closed");
        reject_net(scope, v8::Local::new(scope, &global), &err);
        rv.set(promise.into());
        return;
    };
    tokio::task::spawn_local(async move {
        let result = {
            let mut guard = slot.lock().await;
            crate::ops::tls_write_all(&mut guard, &data).await
        };
        let settler: super::async_ops::Settler = match result {
            Ok(()) => Box::new(move |scope, resolver| {
                let undef = v8::undefined(scope);
                resolver.resolve(scope, undef.into());
            }),
            Err(err) => net_settler_err(err),
        };
        let _ = tx.send(super::async_ops::Completion::new(global, settler));
    });
    rv.set(promise.into());
}

fn op_tls_close<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let stream_id = args.get(0).uint32_value(scope).unwrap_or(0);
    let handle = from_isolate(scope);
    let streams = handle.0.borrow().tls_streams.clone();
    let removed = streams.take(stream_id);
    let was_present = removed.is_some();
    if let Some(slot) = removed {
        tokio::task::spawn_local(async move {
            if let Ok(mut guard) = std::rc::Rc::try_unwrap(slot)
                .map_err(|_| ())
                .map(tokio::sync::Mutex::into_inner)
            {
                let _ = crate::ops::tls_shutdown(&mut guard).await;
            }
        });
    }
    let result = v8::Boolean::new(scope, was_present);
    rv.set(result.into());
}

// ──────────────────────────────────────────────────────────────────────
// node:http / node:https client ops
// ──────────────────────────────────────────────────────────────────────

use crate::ops::{HttpHeader, HttpRequest, http_request};

const HTTP_BRIDGE_TARGET: &str = "nexide::engine::bridge::http";

/// Reads `{ method, url, headers: [[name, value], ...], body: Uint8Array? }`
/// from the JS argument, fires the request asynchronously, and resolves
/// with `{ status, statusText, headers: [[name, value], ...], bodyId }`.
/// `bodyId` is the [`super::bridge::HttpResponseSlot`] handle the JS side
/// uses with `op_http_response_read` until the channel is drained.
fn op_http_request<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Some(resolver) = v8::PromiseResolver::new(scope) else {
        rv.set_undefined();
        return;
    };
    let promise = resolver.get_promise(scope);
    let global = v8::Global::new(scope, resolver);

    let req = match decode_http_request(scope, args.get(0)) {
        Ok(req) => req,
        Err(err) => {
            reject_net(scope, v8::Local::new(scope, &global), &err);
            rv.set(promise.into());
            return;
        }
    };

    let handle = from_isolate(scope);
    let tx = handle.0.borrow().async_completions_tx.clone();
    let table = handle.0.borrow().http_responses.clone();

    tokio::task::spawn_local(async move {
        let settler: super::async_ops::Settler = match http_request(req).await {
            Ok(response) => {
                let id = table.insert(std::rc::Rc::new(tokio::sync::Mutex::new(response.body)));
                let status = response.status;
                let status_text = response.status_text;
                let headers = response.headers;
                tracing::debug!(
                    target: HTTP_BRIDGE_TARGET,
                    body_id = id,
                    status,
                    headers = headers.len(),
                    "http response slot allocated",
                );
                Box::new(move |scope, resolver| {
                    let obj = v8::Object::new(scope);
                    let status_key = v8::String::new(scope, "status").unwrap();
                    let status_val = v8::Number::new(scope, f64::from(status));
                    obj.set(scope, status_key.into(), status_val.into());
                    set_string_field(scope, obj, "statusText", &status_text);
                    let headers_arr = build_header_array(scope, &headers);
                    let headers_key = v8::String::new(scope, "headers").unwrap();
                    obj.set(scope, headers_key.into(), headers_arr.into());
                    let body_key = v8::String::new(scope, "bodyId").unwrap();
                    let body_val = v8::Number::new(scope, f64::from(id));
                    obj.set(scope, body_key.into(), body_val.into());
                    resolver.resolve(scope, obj.into());
                })
            }
            Err(err) => net_settler_err(err),
        };
        let _ = tx.send(super::async_ops::Completion::new(global, settler));
    });
    rv.set(promise.into());
}

/// Pulls one chunk from the response body channel identified by `bodyId`.
/// Resolves with a `Uint8Array` carrying the chunk or `null` once the
/// channel signals end-of-stream.
fn op_http_response_read<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let body_id = args.get(0).uint32_value(scope).unwrap_or(0);
    let Some(resolver) = v8::PromiseResolver::new(scope) else {
        rv.set_undefined();
        return;
    };
    let promise = resolver.get_promise(scope);
    let global = v8::Global::new(scope, resolver);
    let handle = from_isolate(scope);
    let tx = handle.0.borrow().async_completions_tx.clone();
    let bodies = handle.0.borrow().http_responses.clone();
    let Some(slot) = bodies.with(body_id, std::rc::Rc::clone) else {
        let err = NetError::new("EBADF", "http response has been closed");
        reject_net(scope, v8::Local::new(scope, &global), &err);
        rv.set(promise.into());
        return;
    };

    tokio::task::spawn_local(async move {
        let next = {
            let mut guard = slot.lock().await;
            guard.recv().await
        };
        let settler: super::async_ops::Settler = match next {
            Some(Ok(chunk)) => Box::new(move |scope, resolver| {
                let view = bytes_to_uint8_array(scope, &chunk);
                resolver.resolve(scope, view.into());
            }),
            Some(Err(err)) => net_settler_err(err),
            None => Box::new(|scope, resolver| {
                resolver.resolve(scope, v8::null(scope).into());
            }),
        };
        let _ = tx.send(super::async_ops::Completion::new(global, settler));
    });
    rv.set(promise.into());
}

/// Drops the response slot, aborting any in-flight body streaming.
fn op_http_response_close<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let body_id = args.get(0).uint32_value(scope).unwrap_or(0);
    let handle = from_isolate(scope);
    let removed = handle.0.borrow().http_responses.remove(body_id);
    if removed {
        tracing::debug!(
            target: HTTP_BRIDGE_TARGET,
            body_id,
            "http response slot released",
        );
    }
    let result = v8::Boolean::new(scope, removed);
    rv.set(result.into());
}

fn decode_http_request<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    value: v8::Local<'s, v8::Value>,
) -> Result<HttpRequest, NetError> {
    let obj = v8::Local::<v8::Object>::try_from(value)
        .map_err(|_| NetError::new("ERR_INVALID_ARG_TYPE", "request must be an object"))?;
    let method = read_string_field(scope, obj, "method")
        .unwrap_or_else(|| "GET".to_owned())
        .to_uppercase();
    let url = read_string_field(scope, obj, "url")
        .ok_or_else(|| NetError::new("ERR_INVALID_URL", "request.url is required"))?;
    let headers = read_header_array(scope, obj, "headers");
    let body = read_optional_body(scope, obj, "body");
    Ok(HttpRequest {
        method,
        url,
        headers,
        body,
    })
}

fn read_string_field<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    obj: v8::Local<'s, v8::Object>,
    name: &str,
) -> Option<String> {
    let key = v8::String::new(scope, name)?;
    let v = obj.get(scope, key.into())?;
    if v.is_null_or_undefined() {
        return None;
    }
    Some(v.to_rust_string_lossy(scope))
}

fn read_header_array<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    obj: v8::Local<'s, v8::Object>,
    name: &str,
) -> Vec<HttpHeader> {
    let Some(key) = v8::String::new(scope, name) else {
        return Vec::new();
    };
    let Some(value) = obj.get(scope, key.into()) else {
        return Vec::new();
    };
    let Ok(arr) = v8::Local::<v8::Array>::try_from(value) else {
        return Vec::new();
    };
    let len = arr.length();
    let mut out = Vec::with_capacity(len as usize);
    for i in 0..len {
        let Some(entry) = arr.get_index(scope, i) else {
            continue;
        };
        let Ok(pair) = v8::Local::<v8::Array>::try_from(entry) else {
            continue;
        };
        if pair.length() < 2 {
            continue;
        }
        let Some(name_v) = pair.get_index(scope, 0) else {
            continue;
        };
        let Some(value_v) = pair.get_index(scope, 1) else {
            continue;
        };
        out.push(HttpHeader {
            name: name_v.to_rust_string_lossy(scope),
            value: value_v.to_rust_string_lossy(scope),
        });
    }
    out
}

fn read_optional_body<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    obj: v8::Local<'s, v8::Object>,
    name: &str,
) -> Vec<u8> {
    let Some(key) = v8::String::new(scope, name) else {
        return Vec::new();
    };
    let Some(value) = obj.get(scope, key.into()) else {
        return Vec::new();
    };
    if value.is_null_or_undefined() {
        return Vec::new();
    }
    read_uint8_array(scope, value).unwrap_or_default()
}

fn build_header_array<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    headers: &[HttpHeader],
) -> v8::Local<'s, v8::Array> {
    let arr = v8::Array::new(scope, headers.len() as i32);
    for (i, h) in headers.iter().enumerate() {
        let pair = v8::Array::new(scope, 2);
        let name = v8::String::new(scope, &h.name).unwrap_or_else(|| v8::String::empty(scope));
        let value = v8::String::new(scope, &h.value).unwrap_or_else(|| v8::String::empty(scope));
        pair.set_index(scope, 0, name.into());
        pair.set_index(scope, 1, value.into());
        arr.set_index(scope, i as u32, pair.into());
    }
    arr
}

fn bytes_to_uint8_array<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    chunk: &[u8],
) -> v8::Local<'s, v8::Uint8Array> {
    let backing = v8::ArrayBuffer::with_backing_store(
        scope,
        &v8::ArrayBuffer::new_backing_store_from_vec(chunk.to_vec()).make_shared(),
    );
    v8::Uint8Array::new(scope, backing, 0, chunk.len()).expect("Uint8Array view")
}

// ──────────────────────────────────────────────────────────────────────
// node:child_process ops
// ──────────────────────────────────────────────────────────────────────

use super::bridge::ChildSlot;

const PROC_BRIDGE_TARGET: &str = "nexide::engine::bridge::process";
use crate::ops::{
    ExitInfo, SpawnRequest, StdioMode, proc_kill, proc_read_pipe, proc_spawn, proc_wait,
    proc_write_pipe,
};
use std::collections::HashMap;

/// Spawns a child process from the JS descriptor
/// `{ command, args, cwd?, env?, clearEnv?, stdio: ["pipe"|"inherit"|"ignore", ...] }`.
/// Returns `{ pid, id, hasStdin, hasStdout, hasStderr }` on success;
/// rejects with a Node-style error code (e.g. `ENOENT`).
fn op_proc_spawn<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let descriptor = match decode_spawn_descriptor(scope, args.get(0)) {
        Ok(req) => req,
        Err(err) => {
            let exc = make_node_error(scope, &err);
            scope.throw_exception(exc);
            return;
        }
    };
    let handle = from_isolate(scope);
    {
        let state = handle.0.borrow();
        let allowed = state
            .process
            .as_ref()
            .is_none_or(crate::ops::ProcessConfig::subprocess_allowed);
        if !allowed {
            drop(state);
            let err = NetError::new("EPERM", "subprocess spawning is disabled by ProcessConfig");
            let exc = make_node_error(scope, &err);
            scope.throw_exception(exc);
            return;
        }
    }
    let table = handle.0.borrow().child_processes.clone();
    match proc_spawn(descriptor) {
        Ok(child_handle) => {
            let slot = std::rc::Rc::new(ChildSlot {
                child: tokio::sync::Mutex::new(child_handle.child),
                stdin: tokio::sync::Mutex::new(child_handle.stdin),
                stdout: tokio::sync::Mutex::new(child_handle.stdout),
                stderr: tokio::sync::Mutex::new(child_handle.stderr),
            });
            let has_stdin = slot.stdin.try_lock().is_ok_and(|g| g.is_some());
            let has_stdout = slot.stdout.try_lock().is_ok_and(|g| g.is_some());
            let has_stderr = slot.stderr.try_lock().is_ok_and(|g| g.is_some());
            let id = table.insert(slot);
            tracing::debug!(
                target: PROC_BRIDGE_TARGET,
                child_id = id,
                pid = child_handle.pid,
                stdin = has_stdin,
                stdout = has_stdout,
                stderr = has_stderr,
                "child process slot allocated",
            );
            let obj = v8::Object::new(scope);
            let id_key = v8::String::new(scope, "id").unwrap();
            let id_val = v8::Number::new(scope, f64::from(id));
            obj.set(scope, id_key.into(), id_val.into());
            let pid_key = v8::String::new(scope, "pid").unwrap();
            let pid_val = v8::Number::new(scope, f64::from(child_handle.pid));
            obj.set(scope, pid_key.into(), pid_val.into());
            set_bool_field(scope, obj, "hasStdin", has_stdin);
            set_bool_field(scope, obj, "hasStdout", has_stdout);
            set_bool_field(scope, obj, "hasStderr", has_stderr);
            rv.set(obj.into());
        }
        Err(err) => {
            let exc = make_node_error(scope, &err);
            scope.throw_exception(exc);
        }
    }
}

/// Awaits the child's exit. Resolves with `{ code, signal }`.
fn op_proc_wait<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let id = args.get(0).uint32_value(scope).unwrap_or(0);
    let Some(resolver) = v8::PromiseResolver::new(scope) else {
        rv.set_undefined();
        return;
    };
    let promise = resolver.get_promise(scope);
    let global = v8::Global::new(scope, resolver);
    let handle = from_isolate(scope);
    let tx = handle.0.borrow().async_completions_tx.clone();
    let table = handle.0.borrow().child_processes.clone();
    let Some(slot) = table.with(id, std::rc::Rc::clone) else {
        let err = NetError::new("EBADF", "child process has been closed");
        reject_net(scope, v8::Local::new(scope, &global), &err);
        rv.set(promise.into());
        return;
    };
    tokio::task::spawn_local(async move {
        let result = {
            let mut child = slot.child.lock().await;
            proc_wait(&mut child).await
        };
        let settler: super::async_ops::Settler = match result {
            Ok(info) => Box::new(move |scope, resolver| {
                let obj = exit_info_to_object(scope, info);
                resolver.resolve(scope, obj.into());
            }),
            Err(err) => net_settler_err(err),
        };
        let _ = tx.send(super::async_ops::Completion::new(global, settler));
    });
    rv.set(promise.into());
}

/// Sends a signal (or terminates) the child.
fn op_proc_kill<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let id = args.get(0).uint32_value(scope).unwrap_or(0);
    let signal = args.get(1).int32_value(scope).unwrap_or(15);
    let handle = from_isolate(scope);
    let table = handle.0.borrow().child_processes.clone();
    let Some(slot) = table.with(id, std::rc::Rc::clone) else {
        rv.set(v8::Boolean::new(scope, false).into());
        return;
    };
    let result = {
        let mut child_guard = slot.child.try_lock();
        match child_guard.as_mut() {
            Ok(child) => proc_kill(child, signal),
            Err(_) => Err(NetError::new("EBUSY", "child is being awaited")),
        }
    };
    match result {
        Ok(()) => rv.set(v8::Boolean::new(scope, true).into()),
        Err(err) => {
            let exc = make_node_error(scope, &err);
            scope.throw_exception(exc);
        }
    }
}

fn op_proc_stdin_write<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let id = args.get(0).uint32_value(scope).unwrap_or(0);
    let data = match read_uint8_array(scope, args.get(1)) {
        Some(d) => d,
        None => {
            let err = NetError::new("ERR_INVALID_ARG_TYPE", "expected Uint8Array");
            let exc = make_node_error(scope, &err);
            scope.throw_exception(exc);
            return;
        }
    };
    let Some(resolver) = v8::PromiseResolver::new(scope) else {
        rv.set_undefined();
        return;
    };
    let promise = resolver.get_promise(scope);
    let global = v8::Global::new(scope, resolver);
    let handle = from_isolate(scope);
    let tx = handle.0.borrow().async_completions_tx.clone();
    let table = handle.0.borrow().child_processes.clone();
    let Some(slot) = table.with(id, std::rc::Rc::clone) else {
        let err = NetError::new("EBADF", "child process has been closed");
        reject_net(scope, v8::Local::new(scope, &global), &err);
        rv.set(promise.into());
        return;
    };
    tokio::task::spawn_local(async move {
        let result = {
            let mut guard = slot.stdin.lock().await;
            match guard.as_mut() {
                Some(pipe) => proc_write_pipe(pipe, &data).await,
                None => Err(NetError::new("EPIPE", "child stdin is not piped")),
            }
        };
        let settler: super::async_ops::Settler = match result {
            Ok(()) => Box::new(|scope, resolver| {
                resolver.resolve(scope, v8::undefined(scope).into());
            }),
            Err(err) => net_settler_err(err),
        };
        let _ = tx.send(super::async_ops::Completion::new(global, settler));
    });
    rv.set(promise.into());
}

fn op_proc_stdin_close<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let id = args.get(0).uint32_value(scope).unwrap_or(0);
    let handle = from_isolate(scope);
    let table = handle.0.borrow().child_processes.clone();
    let Some(slot) = table.with(id, std::rc::Rc::clone) else {
        rv.set(v8::Boolean::new(scope, false).into());
        return;
    };
    tokio::task::spawn_local(async move {
        let mut guard = slot.stdin.lock().await;
        let _ = guard.take();
    });
    rv.set(v8::Boolean::new(scope, true).into());
}

fn op_proc_stdout_read<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    rv: v8::ReturnValue<'s, v8::Value>,
) {
    proc_pipe_read(scope, args, rv, /*is_stderr=*/ false);
}

fn op_proc_stderr_read<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    rv: v8::ReturnValue<'s, v8::Value>,
) {
    proc_pipe_read(scope, args, rv, /*is_stderr=*/ true);
}

fn proc_pipe_read<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
    is_stderr: bool,
) {
    let id = args.get(0).uint32_value(scope).unwrap_or(0);
    let max = args.get(1).uint32_value(scope).unwrap_or(64 * 1024) as usize;
    let Some(resolver) = v8::PromiseResolver::new(scope) else {
        rv.set_undefined();
        return;
    };
    let promise = resolver.get_promise(scope);
    let global = v8::Global::new(scope, resolver);
    let handle = from_isolate(scope);
    let tx = handle.0.borrow().async_completions_tx.clone();
    let table = handle.0.borrow().child_processes.clone();
    let Some(slot) = table.with(id, std::rc::Rc::clone) else {
        let err = NetError::new("EBADF", "child process has been closed");
        reject_net(scope, v8::Local::new(scope, &global), &err);
        rv.set(promise.into());
        return;
    };
    tokio::task::spawn_local(async move {
        let result = if is_stderr {
            let mut guard = slot.stderr.lock().await;
            match guard.as_mut() {
                Some(pipe) => proc_read_pipe(pipe, max).await,
                None => Ok(None),
            }
        } else {
            let mut guard = slot.stdout.lock().await;
            match guard.as_mut() {
                Some(pipe) => proc_read_pipe(pipe, max).await,
                None => Ok(None),
            }
        };
        let settler: super::async_ops::Settler = match result {
            Ok(Some(chunk)) => Box::new(move |scope, resolver| {
                let view = bytes_to_uint8_array(scope, &chunk);
                resolver.resolve(scope, view.into());
            }),
            Ok(None) => Box::new(|scope, resolver| {
                resolver.resolve(scope, v8::null(scope).into());
            }),
            Err(err) => net_settler_err(err),
        };
        let _ = tx.send(super::async_ops::Completion::new(global, settler));
    });
    rv.set(promise.into());
}

fn op_proc_close<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let id = args.get(0).uint32_value(scope).unwrap_or(0);
    let handle = from_isolate(scope);
    let removed = handle.0.borrow().child_processes.remove(id);
    if removed {
        tracing::debug!(
            target: PROC_BRIDGE_TARGET,
            child_id = id,
            "child process slot released",
        );
    }
    rv.set(v8::Boolean::new(scope, removed).into());
}

fn decode_spawn_descriptor<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    value: v8::Local<'s, v8::Value>,
) -> Result<SpawnRequest, NetError> {
    let obj = v8::Local::<v8::Object>::try_from(value)
        .map_err(|_| NetError::new("ERR_INVALID_ARG_TYPE", "spawn descriptor must be an object"))?;
    let command = read_string_field(scope, obj, "command")
        .ok_or_else(|| NetError::new("ERR_INVALID_ARG_VALUE", "command is required"))?;
    let args = read_string_array(scope, obj, "args");
    let cwd = read_string_field(scope, obj, "cwd");
    let env = read_string_string_record(scope, obj, "env");
    let clear_env = read_bool_field(scope, obj, "clearEnv");
    let stdio = read_stdio_modes(scope, obj, "stdio");
    Ok(SpawnRequest {
        command,
        args,
        cwd,
        env,
        clear_env,
        stdio,
    })
}

fn read_string_array<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    obj: v8::Local<'s, v8::Object>,
    name: &str,
) -> Vec<String> {
    let Some(key) = v8::String::new(scope, name) else {
        return Vec::new();
    };
    let Some(value) = obj.get(scope, key.into()) else {
        return Vec::new();
    };
    let Ok(arr) = v8::Local::<v8::Array>::try_from(value) else {
        return Vec::new();
    };
    let len = arr.length();
    let mut out = Vec::with_capacity(len as usize);
    for i in 0..len {
        if let Some(entry) = arr.get_index(scope, i) {
            out.push(entry.to_rust_string_lossy(scope));
        }
    }
    out
}

fn read_string_string_record<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    obj: v8::Local<'s, v8::Object>,
    name: &str,
) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Some(key) = v8::String::new(scope, name) else {
        return out;
    };
    let Some(value) = obj.get(scope, key.into()) else {
        return out;
    };
    if value.is_null_or_undefined() {
        return out;
    }
    let Ok(record) = v8::Local::<v8::Object>::try_from(value) else {
        return out;
    };
    let Some(names) =
        record.get_own_property_names(scope, v8::GetPropertyNamesArgsBuilder::new().build())
    else {
        return out;
    };
    for i in 0..names.length() {
        let Some(k) = names.get_index(scope, i) else {
            continue;
        };
        let Some(v) = record.get(scope, k) else {
            continue;
        };
        out.insert(k.to_rust_string_lossy(scope), v.to_rust_string_lossy(scope));
    }
    out
}

fn read_bool_field<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    obj: v8::Local<'s, v8::Object>,
    name: &str,
) -> bool {
    let Some(key) = v8::String::new(scope, name) else {
        return false;
    };
    let Some(value) = obj.get(scope, key.into()) else {
        return false;
    };
    value.boolean_value(scope)
}

fn read_stdio_modes<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    obj: v8::Local<'s, v8::Object>,
    name: &str,
) -> [StdioMode; 3] {
    let mut modes = [StdioMode::Pipe, StdioMode::Pipe, StdioMode::Pipe];
    let Some(key) = v8::String::new(scope, name) else {
        return modes;
    };
    let Some(value) = obj.get(scope, key.into()) else {
        return modes;
    };
    if let Ok(arr) = v8::Local::<v8::Array>::try_from(value) {
        let len = arr.length().min(3);
        for i in 0..len {
            if let Some(entry) = arr.get_index(scope, i) {
                modes[i as usize] = parse_stdio_mode(&entry.to_rust_string_lossy(scope));
            }
        }
    }
    modes
}

fn parse_stdio_mode(s: &str) -> StdioMode {
    match s {
        "inherit" => StdioMode::Inherit,
        "ignore" => StdioMode::Ignore,
        _ => StdioMode::Pipe,
    }
}

fn exit_info_to_object<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    info: ExitInfo,
) -> v8::Local<'s, v8::Object> {
    let obj = v8::Object::new(scope);
    let code_key = v8::String::new(scope, "code").unwrap();
    let code_val: v8::Local<v8::Value> = match info.code {
        Some(c) => v8::Number::new(scope, f64::from(c)).into(),
        None => v8::null(scope).into(),
    };
    obj.set(scope, code_key.into(), code_val);
    let signal_key = v8::String::new(scope, "signal").unwrap();
    let signal_val: v8::Local<v8::Value> = match info.signal {
        Some(s) => v8::Number::new(scope, f64::from(s)).into(),
        None => v8::null(scope).into(),
    };
    obj.set(scope, signal_key.into(), signal_val);
    obj
}

fn make_node_error<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    err: &NetError,
) -> v8::Local<'s, v8::Value> {
    let msg = v8::String::new(scope, &err.message).unwrap_or_else(|| v8::String::empty(scope));
    let exc = v8::Exception::error(scope, msg);
    if let Ok(obj) = TryInto::<v8::Local<v8::Object>>::try_into(exc) {
        set_string_field(scope, obj, "code", err.code);
    }
    exc
}

fn set_bool_field<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    obj: v8::Local<'s, v8::Object>,
    name: &str,
    value: bool,
) {
    let key = v8::String::new(scope, name).unwrap();
    let val = v8::Boolean::new(scope, value);
    obj.set(scope, key.into(), val.into());
}

// ──────────────────────────────────────────────────────────────────────
// node:zlib streaming ops
// ──────────────────────────────────────────────────────────────────────

use crate::ops::{ZlibStream, parse_zlib_kind};

const ZLIB_BRIDGE_TARGET: &str = "nexide::engine::bridge::zlib";

/// Creates a streaming zlib state machine. `kind` is the kebab-case
/// identifier (`"deflate"`, `"gunzip"`, …) and `level` is the zlib
/// compression level (0..=9, ignored for decoders).
fn op_zlib_create<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let kind_str = args.get(0).to_rust_string_lossy(scope);
    let level = args.get(1).uint32_value(scope).unwrap_or(6);
    match parse_zlib_kind(&kind_str) {
        Ok(kind) => {
            let stream = ZlibStream::new(kind, level);
            let handle = from_isolate(scope);
            let table = handle.0.borrow().zlib_streams.clone();
            let id = table.insert(std::rc::Rc::new(std::cell::RefCell::new(Some(stream))));
            tracing::debug!(
                target: ZLIB_BRIDGE_TARGET,
                stream_id = id,
                kind = %kind_str,
                level,
                "zlib stream slot allocated",
            );
            rv.set(v8::Number::new(scope, f64::from(id)).into());
        }
        Err(err) => {
            tracing::warn!(
                target: ZLIB_BRIDGE_TARGET,
                kind = %kind_str,
                code = err.code,
                "zlib stream create rejected",
            );
            let exc = make_node_error(scope, &err);
            scope.throw_exception(exc);
        }
    }
}

fn op_zlib_feed<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let id = args.get(0).uint32_value(scope).unwrap_or(0);
    let data = match read_uint8_array(scope, args.get(1)) {
        Some(d) => d,
        None => {
            let err = NetError::new("ERR_INVALID_ARG_TYPE", "expected Uint8Array");
            let exc = make_node_error(scope, &err);
            scope.throw_exception(exc);
            return;
        }
    };
    let handle = from_isolate(scope);
    let table = handle.0.borrow().zlib_streams.clone();
    let Some(slot) = table.with(id, std::rc::Rc::clone) else {
        let err = NetError::new("EBADF", "zlib stream is closed");
        let exc = make_node_error(scope, &err);
        scope.throw_exception(exc);
        return;
    };
    let result = match slot.borrow_mut().as_mut() {
        Some(stream) => stream.feed(&data),
        None => Err(NetError::new("EBADF", "zlib stream is finalised")),
    };
    match result {
        Ok(out) => {
            let view = bytes_to_uint8_array(scope, &out);
            rv.set(view.into());
        }
        Err(err) => {
            let exc = make_node_error(scope, &err);
            scope.throw_exception(exc);
        }
    }
}

fn op_zlib_finish<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let id = args.get(0).uint32_value(scope).unwrap_or(0);
    let handle = from_isolate(scope);
    let table = handle.0.borrow().zlib_streams.clone();
    let Some(slot) = table.with(id, std::rc::Rc::clone) else {
        let err = NetError::new("EBADF", "zlib stream is closed");
        let exc = make_node_error(scope, &err);
        scope.throw_exception(exc);
        return;
    };
    let stream = slot.borrow_mut().take();
    let Some(stream) = stream else {
        let err = NetError::new("EBADF", "zlib stream is already finalised");
        let exc = make_node_error(scope, &err);
        scope.throw_exception(exc);
        return;
    };
    match stream.finish() {
        Ok(out) => {
            let view = bytes_to_uint8_array(scope, &out);
            rv.set(view.into());
        }
        Err(err) => {
            let exc = make_node_error(scope, &err);
            scope.throw_exception(exc);
        }
    }
}

/// Drops the zlib stream slot.
fn op_zlib_close<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let id = args.get(0).uint32_value(scope).unwrap_or(0);
    let handle = from_isolate(scope);
    let removed = handle.0.borrow().zlib_streams.remove(id);
    if removed {
        tracing::debug!(
            target: ZLIB_BRIDGE_TARGET,
            stream_id = id,
            "zlib stream slot released",
        );
    }
    rv.set(v8::Boolean::new(scope, removed).into());
}

// ──────────────────────────────────────────────────────────────────────
// crypto: KDFs, additional ciphers, sign/verify
//
// One-shot Rust ops backed by RustCrypto. The JS shells in
// `polyfills/node/crypto.js` accumulate `update()` chunks and call
// the matching op once during `final()` / `digest()`.
// ──────────────────────────────────────────────────────────────────────

fn op_crypto_pbkdf2<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Some(password) = bytes_arg(scope, &args, 0) else {
        throw_error(scope, "pbkdf2: password must be Uint8Array");
        return;
    };
    let Some(salt) = bytes_arg(scope, &args, 1) else {
        throw_error(scope, "pbkdf2: salt must be Uint8Array");
        return;
    };
    let iterations = args.get(2).uint32_value(scope).unwrap_or(0);
    let keylen = args.get(3).uint32_value(scope).unwrap_or(0) as usize;
    let digest_name = string_arg(scope, &args, 4);
    if iterations == 0 || keylen == 0 {
        throw_error(scope, "pbkdf2: iterations and keylen must be > 0");
        return;
    }
    let mut out = vec![0u8; keylen];
    let result = match digest_name.as_str() {
        "sha1" => pbkdf2::pbkdf2::<hmac::Hmac<sha1::Sha1>>(&password, &salt, iterations, &mut out),
        "sha256" => {
            pbkdf2::pbkdf2::<hmac::Hmac<sha2::Sha256>>(&password, &salt, iterations, &mut out)
        }
        "sha384" => {
            pbkdf2::pbkdf2::<hmac::Hmac<sha2::Sha384>>(&password, &salt, iterations, &mut out)
        }
        "sha512" => {
            pbkdf2::pbkdf2::<hmac::Hmac<sha2::Sha512>>(&password, &salt, iterations, &mut out)
        }
        other => {
            throw_error(scope, &format!("pbkdf2: unsupported digest {other}"));
            return;
        }
    };
    if result.is_err() {
        throw_error(scope, "pbkdf2: invalid key length");
        return;
    }
    let arr = bytes_to_uint8array(scope, &out);
    rv.set(arr.into());
}

fn op_crypto_scrypt<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Some(password) = bytes_arg(scope, &args, 0) else {
        throw_error(scope, "scrypt: password must be Uint8Array");
        return;
    };
    let Some(salt) = bytes_arg(scope, &args, 1) else {
        throw_error(scope, "scrypt: salt must be Uint8Array");
        return;
    };
    let keylen = args.get(2).uint32_value(scope).unwrap_or(0) as usize;
    let n_raw = args.get(3).uint32_value(scope).unwrap_or(16384);
    let r = args.get(4).uint32_value(scope).unwrap_or(8);
    let p = args.get(5).uint32_value(scope).unwrap_or(1);
    if keylen == 0 {
        throw_error(scope, "scrypt: keylen must be > 0");
        return;
    }
    if !n_raw.is_power_of_two() || n_raw < 2 {
        throw_error(scope, "scrypt: N must be a power of two >= 2");
        return;
    }
    let log_n = (31 - n_raw.leading_zeros()) as u8;
    let params = match scrypt::Params::new(log_n, r, p, keylen) {
        Ok(p) => p,
        Err(err) => {
            throw_error(scope, &format!("scrypt: invalid parameters: {err}"));
            return;
        }
    };
    let mut out = vec![0u8; keylen];
    if let Err(err) = scrypt::scrypt(&password, &salt, &params, &mut out) {
        throw_error(scope, &format!("scrypt: derivation failed: {err}"));
        return;
    }
    let arr = bytes_to_uint8array(scope, &out);
    rv.set(arr.into());
}

/// Encrypts a buffer with a non-AEAD AES mode (CBC, CTR).
///
/// `algo` is the Node.js style identifier (e.g. `aes-256-cbc`).
/// CBC requests apply PKCS#7 padding to match Node's default behaviour.
fn op_crypto_aes_encrypt<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    use aes::cipher::{BlockEncryptMut, KeyIvInit, StreamCipher};
    let algo = string_arg(scope, &args, 0);
    let Some(key) = bytes_arg(scope, &args, 1) else {
        throw_error(scope, "aes encrypt: key must be Uint8Array");
        return;
    };
    let Some(iv) = bytes_arg(scope, &args, 2) else {
        throw_error(scope, "aes encrypt: iv must be Uint8Array");
        return;
    };
    let Some(data) = bytes_arg(scope, &args, 3) else {
        throw_error(scope, "aes encrypt: data must be Uint8Array");
        return;
    };
    let result: Result<Vec<u8>, &'static str> = match algo.as_str() {
        "aes-128-cbc" => {
            if key.len() != 16 || iv.len() != 16 {
                Err("aes-128-cbc requires 16-byte key and iv")
            } else {
                let enc = cbc::Encryptor::<aes::Aes128>::new_from_slices(&key, &iv).unwrap();
                Ok(enc.encrypt_padded_vec_mut::<aes::cipher::block_padding::Pkcs7>(&data))
            }
        }
        "aes-192-cbc" => {
            if key.len() != 24 || iv.len() != 16 {
                Err("aes-192-cbc requires 24-byte key and 16-byte iv")
            } else {
                let enc = cbc::Encryptor::<aes::Aes192>::new_from_slices(&key, &iv).unwrap();
                Ok(enc.encrypt_padded_vec_mut::<aes::cipher::block_padding::Pkcs7>(&data))
            }
        }
        "aes-256-cbc" => {
            if key.len() != 32 || iv.len() != 16 {
                Err("aes-256-cbc requires 32-byte key and 16-byte iv")
            } else {
                let enc = cbc::Encryptor::<aes::Aes256>::new_from_slices(&key, &iv).unwrap();
                Ok(enc.encrypt_padded_vec_mut::<aes::cipher::block_padding::Pkcs7>(&data))
            }
        }
        "aes-128-ctr" => {
            if key.len() != 16 || iv.len() != 16 {
                Err("aes-128-ctr requires 16-byte key and iv")
            } else {
                let mut buf = data.clone();
                let mut c = ctr::Ctr128BE::<aes::Aes128>::new_from_slices(&key, &iv).unwrap();
                c.apply_keystream(&mut buf);
                Ok(buf)
            }
        }
        "aes-256-ctr" => {
            if key.len() != 32 || iv.len() != 16 {
                Err("aes-256-ctr requires 32-byte key and 16-byte iv")
            } else {
                let mut buf = data.clone();
                let mut c = ctr::Ctr128BE::<aes::Aes256>::new_from_slices(&key, &iv).unwrap();
                c.apply_keystream(&mut buf);
                Ok(buf)
            }
        }
        other => Err(Box::leak(
            format!("unsupported cipher {other}").into_boxed_str(),
        )),
    };
    match result {
        Ok(out) => {
            let arr = bytes_to_uint8array(scope, &out);
            rv.set(arr.into());
        }
        Err(msg) => throw_error(scope, msg),
    }
}

fn op_crypto_aes_decrypt<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    use aes::cipher::{BlockDecryptMut, KeyIvInit, StreamCipher};
    let algo = string_arg(scope, &args, 0);
    let Some(key) = bytes_arg(scope, &args, 1) else {
        throw_error(scope, "aes decrypt: key must be Uint8Array");
        return;
    };
    let Some(iv) = bytes_arg(scope, &args, 2) else {
        throw_error(scope, "aes decrypt: iv must be Uint8Array");
        return;
    };
    let Some(data) = bytes_arg(scope, &args, 3) else {
        throw_error(scope, "aes decrypt: data must be Uint8Array");
        return;
    };
    let result: Result<Vec<u8>, String> = match algo.as_str() {
        "aes-128-cbc" => cbc::Decryptor::<aes::Aes128>::new_from_slices(&key, &iv)
            .map_err(|e| e.to_string())
            .and_then(|dec| {
                dec.decrypt_padded_vec_mut::<aes::cipher::block_padding::Pkcs7>(&data)
                    .map_err(|e| e.to_string())
            }),
        "aes-192-cbc" => cbc::Decryptor::<aes::Aes192>::new_from_slices(&key, &iv)
            .map_err(|e| e.to_string())
            .and_then(|dec| {
                dec.decrypt_padded_vec_mut::<aes::cipher::block_padding::Pkcs7>(&data)
                    .map_err(|e| e.to_string())
            }),
        "aes-256-cbc" => cbc::Decryptor::<aes::Aes256>::new_from_slices(&key, &iv)
            .map_err(|e| e.to_string())
            .and_then(|dec| {
                dec.decrypt_padded_vec_mut::<aes::cipher::block_padding::Pkcs7>(&data)
                    .map_err(|e| e.to_string())
            }),
        "aes-128-ctr" => ctr::Ctr128BE::<aes::Aes128>::new_from_slices(&key, &iv)
            .map_err(|e| e.to_string())
            .map(|mut c| {
                let mut buf = data.clone();
                c.apply_keystream(&mut buf);
                buf
            }),
        "aes-256-ctr" => ctr::Ctr128BE::<aes::Aes256>::new_from_slices(&key, &iv)
            .map_err(|e| e.to_string())
            .map(|mut c| {
                let mut buf = data.clone();
                c.apply_keystream(&mut buf);
                buf
            }),
        other => Err(format!("unsupported cipher {other}")),
    };
    match result {
        Ok(out) => {
            let arr = bytes_to_uint8array(scope, &out);
            rv.set(arr.into());
        }
        Err(err) => throw_error(scope, &err),
    }
}

/// AEAD seal with `chacha20-poly1305`. Output is `ciphertext || tag(16)`.
fn op_crypto_chacha20_seal<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    use chacha20poly1305::{ChaCha20Poly1305, KeyInit, aead::Aead, aead::Payload};
    let Some(key) = bytes_arg(scope, &args, 0) else {
        throw_error(scope, "chacha20 seal: key must be Uint8Array");
        return;
    };
    let Some(nonce) = bytes_arg(scope, &args, 1) else {
        throw_error(scope, "chacha20 seal: nonce must be Uint8Array");
        return;
    };
    let Some(plaintext) = bytes_arg(scope, &args, 2) else {
        throw_error(scope, "chacha20 seal: plaintext must be Uint8Array");
        return;
    };
    let aad = bytes_arg(scope, &args, 3).unwrap_or_default();
    if key.len() != 32 || nonce.len() != 12 {
        throw_error(
            scope,
            "chacha20-poly1305 requires 32-byte key and 12-byte nonce",
        );
        return;
    }
    let cipher = ChaCha20Poly1305::new_from_slice(&key).unwrap();
    let nonce_arr = chacha20poly1305::Nonce::from_slice(&nonce);
    match cipher.encrypt(
        nonce_arr,
        Payload {
            msg: &plaintext,
            aad: &aad,
        },
    ) {
        Ok(out) => {
            let arr = bytes_to_uint8array(scope, &out);
            rv.set(arr.into());
        }
        Err(err) => throw_error(scope, &format!("chacha20 seal: {err}")),
    }
}

fn op_crypto_chacha20_open<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    use chacha20poly1305::{ChaCha20Poly1305, KeyInit, aead::Aead, aead::Payload};
    let Some(key) = bytes_arg(scope, &args, 0) else {
        throw_error(scope, "chacha20 open: key must be Uint8Array");
        return;
    };
    let Some(nonce) = bytes_arg(scope, &args, 1) else {
        throw_error(scope, "chacha20 open: nonce must be Uint8Array");
        return;
    };
    let Some(ciphertext) = bytes_arg(scope, &args, 2) else {
        throw_error(scope, "chacha20 open: ciphertext must be Uint8Array");
        return;
    };
    let aad = bytes_arg(scope, &args, 3).unwrap_or_default();
    if key.len() != 32 || nonce.len() != 12 {
        throw_error(
            scope,
            "chacha20-poly1305 requires 32-byte key and 12-byte nonce",
        );
        return;
    }
    let cipher = ChaCha20Poly1305::new_from_slice(&key).unwrap();
    let nonce_arr = chacha20poly1305::Nonce::from_slice(&nonce);
    match cipher.decrypt(
        nonce_arr,
        Payload {
            msg: &ciphertext,
            aad: &aad,
        },
    ) {
        Ok(out) => {
            let arr = bytes_to_uint8array(scope, &out);
            rv.set(arr.into());
        }
        Err(err) => throw_error(scope, &format!("chacha20 open: {err}")),
    }
}

/// Signs a message with a PEM-encoded private key.
///
/// Supported algorithms (Node-style identifiers):
///   * `rsa-sha256`, `rsa-sha384`, `rsa-sha512` - RSASSA-PKCS1-v1_5
///   * `ecdsa-p256-sha256` - DER-encoded ECDSA on the P-256 curve
///   * `ed25519` - pure EdDSA, no prehash
fn op_crypto_sign<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let algo = string_arg(scope, &args, 0);
    let key_pem = string_arg(scope, &args, 1);
    let Some(data) = bytes_arg(scope, &args, 2) else {
        throw_error(scope, "sign: data must be Uint8Array");
        return;
    };
    let result: Result<Vec<u8>, String> = match algo.as_str() {
        "rsa-sha256" => rsa_sign::<sha2::Sha256>(&key_pem, &data),
        "rsa-sha384" => rsa_sign::<sha2::Sha384>(&key_pem, &data),
        "rsa-sha512" => rsa_sign::<sha2::Sha512>(&key_pem, &data),
        "ecdsa-p256-sha256" => ecdsa_p256_sign(&key_pem, &data),
        "ed25519" => ed25519_sign(&key_pem, &data),
        other => Err(format!("unsupported sign algorithm: {other}")),
    };
    match result {
        Ok(sig) => {
            let arr = bytes_to_uint8array(scope, &sig);
            rv.set(arr.into());
        }
        Err(err) => throw_error(scope, &err),
    }
}

fn op_crypto_verify<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let algo = string_arg(scope, &args, 0);
    let key_pem = string_arg(scope, &args, 1);
    let Some(data) = bytes_arg(scope, &args, 2) else {
        throw_error(scope, "verify: data must be Uint8Array");
        return;
    };
    let Some(signature) = bytes_arg(scope, &args, 3) else {
        throw_error(scope, "verify: signature must be Uint8Array");
        return;
    };
    let result: Result<bool, String> = match algo.as_str() {
        "rsa-sha256" => rsa_verify::<sha2::Sha256>(&key_pem, &data, &signature),
        "rsa-sha384" => rsa_verify::<sha2::Sha384>(&key_pem, &data, &signature),
        "rsa-sha512" => rsa_verify::<sha2::Sha512>(&key_pem, &data, &signature),
        "ecdsa-p256-sha256" => ecdsa_p256_verify(&key_pem, &data, &signature),
        "ed25519" => ed25519_verify(&key_pem, &data, &signature),
        other => Err(format!("unsupported verify algorithm: {other}")),
    };
    match result {
        Ok(ok) => rv.set(v8::Boolean::new(scope, ok).into()),
        Err(err) => throw_error(scope, &err),
    }
}

fn rsa_sign<D>(key_pem: &str, data: &[u8]) -> Result<Vec<u8>, String>
where
    D: digest::Digest + digest::const_oid::AssociatedOid,
{
    use rsa::pkcs1v15::SigningKey;
    use rsa::pkcs8::DecodePrivateKey;
    use rsa::signature::{SignatureEncoding, Signer};
    let key = rsa::RsaPrivateKey::from_pkcs8_pem(key_pem)
        .or_else(|_| {
            use rsa::pkcs1::DecodeRsaPrivateKey;
            rsa::RsaPrivateKey::from_pkcs1_pem(key_pem)
        })
        .map_err(|e| format!("rsa private key parse: {e}"))?;
    let signing_key = SigningKey::<D>::new(key);
    let sig = signing_key.sign(data);
    Ok(sig.to_bytes().into_vec())
}

fn rsa_verify<D>(key_pem: &str, data: &[u8], sig: &[u8]) -> Result<bool, String>
where
    D: digest::Digest + digest::const_oid::AssociatedOid,
{
    use rsa::pkcs1v15::{Signature, VerifyingKey};
    use rsa::pkcs8::DecodePublicKey;
    use rsa::signature::Verifier;
    let pub_key = rsa::RsaPublicKey::from_public_key_pem(key_pem)
        .or_else(|_| {
            use rsa::pkcs1::DecodeRsaPublicKey;
            rsa::RsaPublicKey::from_pkcs1_pem(key_pem)
        })
        .map_err(|e| format!("rsa public key parse: {e}"))?;
    let verifying = VerifyingKey::<D>::new(pub_key);
    let signature = Signature::try_from(sig).map_err(|e| format!("rsa signature decode: {e}"))?;
    Ok(verifying.verify(data, &signature).is_ok())
}

fn ecdsa_p256_sign(key_pem: &str, data: &[u8]) -> Result<Vec<u8>, String> {
    use p256::ecdsa::signature::Signer;
    use p256::ecdsa::{Signature, SigningKey};
    use p256::pkcs8::DecodePrivateKey;
    let signing =
        SigningKey::from_pkcs8_pem(key_pem).map_err(|e| format!("p256 private key parse: {e}"))?;
    let sig: Signature = signing.sign(data);
    Ok(sig.to_der().as_bytes().to_vec())
}

fn ecdsa_p256_verify(key_pem: &str, data: &[u8], sig: &[u8]) -> Result<bool, String> {
    use p256::ecdsa::signature::Verifier;
    use p256::ecdsa::{Signature, VerifyingKey};
    use p256::pkcs8::DecodePublicKey;
    let verifying = VerifyingKey::from_public_key_pem(key_pem)
        .map_err(|e| format!("p256 public key parse: {e}"))?;
    let signature = Signature::from_der(sig)
        .or_else(|_| Signature::try_from(sig))
        .map_err(|e| format!("p256 signature decode: {e}"))?;
    Ok(verifying.verify(data, &signature).is_ok())
}

fn ed25519_sign(key_pem: &str, data: &[u8]) -> Result<Vec<u8>, String> {
    use ed25519_dalek::Signer;
    use ed25519_dalek::SigningKey;
    use ed25519_dalek::pkcs8::DecodePrivateKey;
    let signing = SigningKey::from_pkcs8_pem(key_pem)
        .map_err(|e| format!("ed25519 private key parse: {e}"))?;
    let sig = signing.sign(data);
    Ok(sig.to_bytes().to_vec())
}

fn ed25519_verify(key_pem: &str, data: &[u8], sig: &[u8]) -> Result<bool, String> {
    use ed25519_dalek::pkcs8::DecodePublicKey;
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    let verifying = VerifyingKey::from_public_key_pem(key_pem)
        .map_err(|e| format!("ed25519 public key parse: {e}"))?;
    if sig.len() != 64 {
        return Err(format!(
            "ed25519 signature must be 64 bytes, got {}",
            sig.len()
        ));
    }
    let mut sig_arr = [0u8; 64];
    sig_arr.copy_from_slice(sig);
    let signature = Signature::from_bytes(&sig_arr);
    Ok(verifying.verify(data, &signature).is_ok())
}

fn op_crypto_pem_decode<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let pem_str = string_arg(scope, &args, 0);
    match pem::parse(&pem_str) {
        Ok(parsed) => {
            let obj = v8::Object::new(scope);
            let label_key = v8::String::new(scope, "label").unwrap();
            let label_val = v8::String::new(scope, parsed.tag()).unwrap();
            obj.set(scope, label_key.into(), label_val.into());
            let der_key = v8::String::new(scope, "der").unwrap();
            let der_arr = bytes_to_uint8array(scope, parsed.contents());
            obj.set(scope, der_key.into(), der_arr.into());
            rv.set(obj.into());
        }
        Err(e) => throw_error(scope, &format!("pem_decode: {e}")),
    }
}

fn op_crypto_pem_encode<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let label = string_arg(scope, &args, 0);
    let Some(der) = bytes_arg(scope, &args, 1) else {
        throw_error(scope, "pem_encode: der must be Uint8Array");
        return;
    };
    let pem_obj = pem::Pem::new(&label, der);
    let config = pem::EncodeConfig::new().set_line_ending(pem::LineEnding::LF);
    let encoded = pem::encode_config(&pem_obj, config);
    let s = v8::String::new(scope, &encoded).unwrap();
    rv.set(s.into());
}

fn op_crypto_generate_key_pair<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let key_type = string_arg(scope, &args, 0);
    let options_json = string_arg(scope, &args, 1);
    let options: serde_json::Value = serde_json::from_str(&options_json).unwrap_or_default();
    let result = generate_key_pair_impl(&key_type, &options);
    match result {
        Ok((pub_der, priv_der, info_json)) => {
            let obj = v8::Object::new(scope);
            let pub_key = v8::String::new(scope, "publicKey").unwrap();
            let pub_arr = bytes_to_uint8array(scope, &pub_der);
            obj.set(scope, pub_key.into(), pub_arr.into());
            let priv_key = v8::String::new(scope, "privateKey").unwrap();
            let priv_arr = bytes_to_uint8array(scope, &priv_der);
            obj.set(scope, priv_key.into(), priv_arr.into());
            let info_key = v8::String::new(scope, "info_json").unwrap();
            let info_val = v8::String::new(scope, &info_json).unwrap();
            obj.set(scope, info_key.into(), info_val.into());
            rv.set(obj.into());
        }
        Err(e) => throw_error(scope, &e),
    }
}

fn generate_key_pair_impl(
    key_type: &str,
    options: &serde_json::Value,
) -> Result<(Vec<u8>, Vec<u8>, String), String> {
    use pkcs8::{EncodePrivateKey, EncodePublicKey};
    use rand_core::{OsRng, RngCore};
    match key_type {
        "rsa" | "rsa-pss" => {
            let modulus_length = options["modulusLength"].as_u64().unwrap_or(2048) as usize;
            let public_exponent = options["publicExponent"].as_u64().unwrap_or(65537);
            let mut rng = OsRng;
            let bits = modulus_length;
            let priv_key = rsa::RsaPrivateKey::new_with_exp(
                &mut rng,
                bits,
                &rsa::BigUint::from(public_exponent),
            )
            .map_err(|e| format!("rsa keygen: {e}"))?;
            let pub_key = priv_key.to_public_key();
            let priv_der = priv_key
                .to_pkcs8_der()
                .map_err(|e| format!("rsa priv pkcs8: {e}"))?
                .as_bytes()
                .to_vec();
            let pub_der = pub_key
                .to_public_key_der()
                .map_err(|e| format!("rsa pub spki: {e}"))?
                .as_bytes()
                .to_vec();
            let info = serde_json::json!({
                "asymmetricKeyType": key_type,
                "modulusLength": modulus_length,
                "publicExponent": public_exponent,
            });
            Ok((pub_der, priv_der, info.to_string()))
        }
        "ec" => {
            let curve_name = options["namedCurve"].as_str().unwrap_or("prime256v1");
            let (pub_der, priv_der) = match curve_name {
                "prime256v1" | "P-256" => {
                    let signing = p256::ecdsa::SigningKey::random(&mut OsRng);
                    let priv_der = signing
                        .to_pkcs8_der()
                        .map_err(|e| format!("p256 priv: {e}"))?
                        .as_bytes()
                        .to_vec();
                    let pub_der = signing
                        .verifying_key()
                        .to_public_key_der()
                        .map_err(|e| format!("p256 pub: {e}"))?
                        .as_bytes()
                        .to_vec();
                    (pub_der, priv_der)
                }
                "secp384r1" | "P-384" => {
                    let signing = p384::ecdsa::SigningKey::random(&mut OsRng);
                    let priv_der = signing
                        .to_pkcs8_der()
                        .map_err(|e| format!("p384 priv: {e}"))?
                        .as_bytes()
                        .to_vec();
                    let pub_der = signing
                        .verifying_key()
                        .to_public_key_der()
                        .map_err(|e| format!("p384 pub: {e}"))?
                        .as_bytes()
                        .to_vec();
                    (pub_der, priv_der)
                }
                "secp521r1" | "P-521" => {
                    let secret = p521::SecretKey::random(&mut OsRng);
                    let public = secret.public_key();
                    let priv_der = secret
                        .to_pkcs8_der()
                        .map_err(|e| format!("p521 priv: {e}"))?
                        .as_bytes()
                        .to_vec();
                    let pub_der = public
                        .to_public_key_der()
                        .map_err(|e| format!("p521 pub: {e}"))?
                        .as_bytes()
                        .to_vec();
                    (pub_der, priv_der)
                }
                other => return Err(format!("unsupported curve: {other}")),
            };
            let info = serde_json::json!({
                "asymmetricKeyType": "ec",
                "namedCurve": curve_name,
            });
            Ok((pub_der, priv_der, info.to_string()))
        }
        "ed25519" => {
            use ed25519_dalek::SigningKey;
            let mut seed = [0u8; 32];
            OsRng.fill_bytes(&mut seed);
            let signing = SigningKey::from_bytes(&seed);
            let priv_der = signing
                .to_pkcs8_der()
                .map_err(|e| format!("ed25519 priv: {e}"))?
                .as_bytes()
                .to_vec();
            let pub_der = signing
                .verifying_key()
                .to_public_key_der()
                .map_err(|e| format!("ed25519 pub: {e}"))?
                .as_bytes()
                .to_vec();
            let info = serde_json::json!({"asymmetricKeyType": "ed25519"});
            Ok((pub_der, priv_der, info.to_string()))
        }
        "x25519" => {
            use x25519_dalek::StaticSecret;
            let secret = StaticSecret::random_from_rng(OsRng);
            let public = x25519_dalek::PublicKey::from(&secret);
            let priv_der =
                x25519_secret_to_pkcs8(&secret).map_err(|e| format!("x25519 priv: {e}"))?;
            let pub_der = x25519_public_to_spki(&public).map_err(|e| format!("x25519 pub: {e}"))?;
            let info = serde_json::json!({"asymmetricKeyType": "x25519"});
            Ok((pub_der, priv_der, info.to_string()))
        }
        other => Err(format!(
            "unsupported key type: {other}. Supported: rsa, rsa-pss, ec, ed25519, x25519"
        )),
    }
}

fn x25519_secret_to_pkcs8(secret: &x25519_dalek::StaticSecret) -> Result<Vec<u8>, String> {
    use pkcs8::{PrivateKeyInfo, der::Encode};
    let secret_bytes = secret.to_bytes();
    let oid = pkcs8::ObjectIdentifier::new_unwrap("1.3.101.110");
    let alg = pkcs8::AlgorithmIdentifierRef {
        oid,
        parameters: None,
    };
    let pki = PrivateKeyInfo::new(alg, &secret_bytes);
    pki.to_der()
        .map_err(|e| format!("x25519 pkcs8 encode: {e}"))
}

fn x25519_public_to_spki(public: &x25519_dalek::PublicKey) -> Result<Vec<u8>, String> {
    use spki::{SubjectPublicKeyInfoOwned, der::Encode};
    let oid = spki::ObjectIdentifier::new_unwrap("1.3.101.110");
    let alg = spki::AlgorithmIdentifierOwned {
        oid,
        parameters: None,
    };
    let spki = SubjectPublicKeyInfoOwned {
        algorithm: alg,
        subject_public_key: spki::der::asn1::BitString::from_bytes(public.as_bytes())
            .map_err(|e| format!("x25519 bitstring: {e}"))?,
    };
    spki.to_der()
        .map_err(|e| format!("x25519 spki encode: {e}"))
}

fn biguint_to_u64(n: &rsa::BigUint) -> u64 {
    if n.bits() <= 64 {
        let bytes = n.to_bytes_be();
        let mut arr = [0u8; 8];
        let start = 8usize.saturating_sub(bytes.len());
        arr[start..].copy_from_slice(&bytes[..bytes.len().min(8)]);
        u64::from_be_bytes(arr)
    } else {
        65537
    }
}

fn op_crypto_key_inspect<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Some(der) = bytes_arg(scope, &args, 0) else {
        throw_error(scope, "key_inspect: der must be Uint8Array");
        return;
    };
    let kind = string_arg(scope, &args, 1);
    let result = key_inspect_impl(&der, &kind);
    match result {
        Ok(json_str) => {
            let s = v8::String::new(scope, &json_str).unwrap();
            rv.set(s.into());
        }
        Err(e) => throw_error(scope, &e),
    }
}

fn key_inspect_impl(der: &[u8], kind: &str) -> Result<String, String> {
    use pkcs8::{DecodePrivateKey, DecodePublicKey};
    use rsa::traits::PublicKeyParts;
    match kind {
        "private-pkcs8" => {
            if let Ok(rsa_key) = rsa::RsaPrivateKey::from_pkcs8_der(der) {
                let mod_bits = rsa_key.n().bits();
                let exp_u64 = if rsa_key.e().bits() <= 64 {
                    let bytes = rsa_key.e().to_bytes_be();
                    let mut arr = [0u8; 8];
                    let start = 8usize.saturating_sub(bytes.len());
                    arr[start..].copy_from_slice(&bytes[..bytes.len().min(8)]);
                    u64::from_be_bytes(arr)
                } else {
                    65537
                };
                let info = serde_json::json!({
                    "asymmetricKeyType": "rsa",
                    "modulusLength": mod_bits,
                    "publicExponent": exp_u64,
                });
                return Ok(info.to_string());
            }
            if p256::SecretKey::from_pkcs8_der(der).is_ok() {
                let info = serde_json::json!({
                    "asymmetricKeyType": "ec",
                    "namedCurve": "prime256v1",
                });
                return Ok(info.to_string());
            }
            if p384::SecretKey::from_pkcs8_der(der).is_ok() {
                let info = serde_json::json!({
                    "asymmetricKeyType": "ec",
                    "namedCurve": "secp384r1",
                });
                return Ok(info.to_string());
            }
            if p521::SecretKey::from_pkcs8_der(der).is_ok() {
                let info = serde_json::json!({
                    "asymmetricKeyType": "ec",
                    "namedCurve": "secp521r1",
                });
                return Ok(info.to_string());
            }
            if ed25519_dalek::SigningKey::from_pkcs8_der(der).is_ok() {
                let info = serde_json::json!({"asymmetricKeyType": "ed25519"});
                return Ok(info.to_string());
            }
            if x25519_pkcs8_to_secret(der).is_ok() {
                let info = serde_json::json!({"asymmetricKeyType": "x25519"});
                return Ok(info.to_string());
            }
            Err("private-pkcs8: unsupported key type".to_string())
        }
        "public-spki" => {
            if let Ok(rsa_key) = rsa::RsaPublicKey::from_public_key_der(der) {
                let info = serde_json::json!({
                    "asymmetricKeyType": "rsa",
                    "modulusLength": rsa_key.n().bits(),
                    "publicExponent": biguint_to_u64(rsa_key.e()),
                });
                return Ok(info.to_string());
            }
            if p256::PublicKey::from_public_key_der(der).is_ok() {
                let info = serde_json::json!({
                    "asymmetricKeyType": "ec",
                    "namedCurve": "prime256v1",
                });
                return Ok(info.to_string());
            }
            if p384::PublicKey::from_public_key_der(der).is_ok() {
                let info = serde_json::json!({
                    "asymmetricKeyType": "ec",
                    "namedCurve": "secp384r1",
                });
                return Ok(info.to_string());
            }
            if p521::PublicKey::from_public_key_der(der).is_ok() {
                let info = serde_json::json!({
                    "asymmetricKeyType": "ec",
                    "namedCurve": "secp521r1",
                });
                return Ok(info.to_string());
            }
            if ed25519_dalek::VerifyingKey::from_public_key_der(der).is_ok() {
                let info = serde_json::json!({"asymmetricKeyType": "ed25519"});
                return Ok(info.to_string());
            }
            if x25519_spki_to_public(der).is_ok() {
                let info = serde_json::json!({"asymmetricKeyType": "x25519"});
                return Ok(info.to_string());
            }
            Err("public-spki: unsupported key type".to_string())
        }
        "rsa-pkcs1-priv" => {
            use pkcs1::DecodeRsaPrivateKey;
            let rsa_key = rsa::RsaPrivateKey::from_pkcs1_der(der)
                .map_err(|e| format!("rsa-pkcs1-priv parse: {e}"))?;
            let info = serde_json::json!({
                "asymmetricKeyType": "rsa",
                "modulusLength": rsa_key.n().bits(),
                "publicExponent": biguint_to_u64(rsa_key.e()),
            });
            Ok(info.to_string())
        }
        "rsa-pkcs1-pub" => {
            use pkcs1::DecodeRsaPublicKey;
            let rsa_key = rsa::RsaPublicKey::from_pkcs1_der(der)
                .map_err(|e| format!("rsa-pkcs1-pub parse: {e}"))?;
            let info = serde_json::json!({
                "asymmetricKeyType": "rsa",
                "modulusLength": rsa_key.n().bits(),
                "publicExponent": biguint_to_u64(rsa_key.e()),
            });
            Ok(info.to_string())
        }
        "ec-sec1" => {
            if p256::SecretKey::from_sec1_der(der).is_ok() {
                let info = serde_json::json!({
                    "asymmetricKeyType": "ec",
                    "namedCurve": "prime256v1",
                });
                return Ok(info.to_string());
            }
            if p384::SecretKey::from_sec1_der(der).is_ok() {
                let info = serde_json::json!({
                    "asymmetricKeyType": "ec",
                    "namedCurve": "secp384r1",
                });
                return Ok(info.to_string());
            }
            if p521::SecretKey::from_sec1_der(der).is_ok() {
                let info = serde_json::json!({
                    "asymmetricKeyType": "ec",
                    "namedCurve": "secp521r1",
                });
                return Ok(info.to_string());
            }
            Err("ec-sec1: unsupported curve".to_string())
        }
        other => Err(format!("unsupported kind: {other}")),
    }
}

fn x25519_pkcs8_to_secret(der: &[u8]) -> Result<x25519_dalek::StaticSecret, String> {
    use pkcs8::{PrivateKeyInfo, der::Decode};
    let pki = PrivateKeyInfo::from_der(der).map_err(|e| format!("pkcs8 decode: {e}"))?;
    if pki.private_key.len() != 32 {
        return Err(format!(
            "x25519 secret must be 32 bytes, got {}",
            pki.private_key.len()
        ));
    }
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(pki.private_key);
    Ok(x25519_dalek::StaticSecret::from(bytes))
}

fn x25519_spki_to_public(der: &[u8]) -> Result<x25519_dalek::PublicKey, String> {
    use spki::{SubjectPublicKeyInfoRef, der::Decode};
    let spki = SubjectPublicKeyInfoRef::from_der(der).map_err(|e| format!("spki decode: {e}"))?;
    let key_bytes = spki.subject_public_key.raw_bytes();
    if key_bytes.len() != 32 {
        return Err(format!(
            "x25519 public must be 32 bytes, got {}",
            key_bytes.len()
        ));
    }
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(key_bytes);
    Ok(x25519_dalek::PublicKey::from(bytes))
}

fn op_crypto_key_convert<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let input_kind = string_arg(scope, &args, 0);
    let output_kind = string_arg(scope, &args, 1);
    let Some(der) = bytes_arg(scope, &args, 2) else {
        throw_error(scope, "key_convert: der must be Uint8Array");
        return;
    };
    let curve_hint = if args.length() >= 4 {
        Some(string_arg(scope, &args, 3))
    } else {
        None
    };
    let result = key_convert_impl(&input_kind, &output_kind, &der, curve_hint.as_deref());
    match result {
        Ok(out_der) => {
            let arr = bytes_to_uint8array(scope, &out_der);
            rv.set(arr.into());
        }
        Err(e) => throw_error(scope, &e),
    }
}

fn key_convert_impl(
    input_kind: &str,
    output_kind: &str,
    der: &[u8],
    curve_hint: Option<&str>,
) -> Result<Vec<u8>, String> {
    use pkcs1::{DecodeRsaPrivateKey, DecodeRsaPublicKey, EncodeRsaPrivateKey, EncodeRsaPublicKey};
    use pkcs8::{DecodePrivateKey, DecodePublicKey, EncodePrivateKey, EncodePublicKey};
    match (input_kind, output_kind) {
        ("pkcs1-priv", "private-pkcs8") => {
            let rsa_key = rsa::RsaPrivateKey::from_pkcs1_der(der)
                .map_err(|e| format!("pkcs1-priv parse: {e}"))?;
            rsa_key
                .to_pkcs8_der()
                .map(|d| d.as_bytes().to_vec())
                .map_err(|e| format!("pkcs8 encode: {e}"))
        }
        ("private-pkcs8", "pkcs1-priv") => {
            let rsa_key =
                rsa::RsaPrivateKey::from_pkcs8_der(der).map_err(|e| format!("pkcs8 parse: {e}"))?;
            rsa_key
                .to_pkcs1_der()
                .map(|d| d.as_bytes().to_vec())
                .map_err(|e| format!("pkcs1 encode: {e}"))
        }
        ("pkcs1-pub", "public-spki") => {
            let rsa_key = rsa::RsaPublicKey::from_pkcs1_der(der)
                .map_err(|e| format!("pkcs1-pub parse: {e}"))?;
            rsa_key
                .to_public_key_der()
                .map(|d| d.as_bytes().to_vec())
                .map_err(|e| format!("spki encode: {e}"))
        }
        ("public-spki", "pkcs1-pub") => {
            let rsa_key = rsa::RsaPublicKey::from_public_key_der(der)
                .map_err(|e| format!("spki parse: {e}"))?;
            rsa_key
                .to_pkcs1_der()
                .map(|d| d.as_bytes().to_vec())
                .map_err(|e| format!("pkcs1 encode: {e}"))
        }
        ("ec-sec1", "private-pkcs8") => {
            let curve = curve_hint.unwrap_or("prime256v1");
            match curve {
                "prime256v1" | "P-256" => {
                    let sk = p256::SecretKey::from_sec1_der(der)
                        .map_err(|e| format!("p256 sec1: {e}"))?;
                    sk.to_pkcs8_der()
                        .map(|d| d.as_bytes().to_vec())
                        .map_err(|e| format!("p256 pkcs8: {e}"))
                }
                "secp384r1" | "P-384" => {
                    let sk = p384::SecretKey::from_sec1_der(der)
                        .map_err(|e| format!("p384 sec1: {e}"))?;
                    sk.to_pkcs8_der()
                        .map(|d| d.as_bytes().to_vec())
                        .map_err(|e| format!("p384 pkcs8: {e}"))
                }
                "secp521r1" | "P-521" => {
                    let sk = p521::SecretKey::from_sec1_der(der)
                        .map_err(|e| format!("p521 sec1: {e}"))?;
                    sk.to_pkcs8_der()
                        .map(|d| d.as_bytes().to_vec())
                        .map_err(|e| format!("p521 pkcs8: {e}"))
                }
                other => Err(format!("unsupported curve for sec1: {other}")),
            }
        }
        ("private-pkcs8", "ec-sec1") => {
            if let Ok(sk) = p256::SecretKey::from_pkcs8_der(der) {
                return sk
                    .to_sec1_der()
                    .map(|d| (*d).clone())
                    .map_err(|e| format!("p256 sec1: {e}"));
            }
            if let Ok(sk) = p384::SecretKey::from_pkcs8_der(der) {
                return sk
                    .to_sec1_der()
                    .map(|d| (*d).clone())
                    .map_err(|e| format!("p384 sec1: {e}"));
            }
            if let Ok(sk) = p521::SecretKey::from_pkcs8_der(der) {
                return sk
                    .to_sec1_der()
                    .map(|d| (*d).clone())
                    .map_err(|e| format!("p521 sec1: {e}"));
            }
            Err("private-pkcs8 -> ec-sec1: not an EC key".to_string())
        }
        ("private-pkcs8", "public-spki") => {
            if let Ok(rsa_key) = rsa::RsaPrivateKey::from_pkcs8_der(der) {
                return rsa_key
                    .to_public_key()
                    .to_public_key_der()
                    .map(|d| d.as_bytes().to_vec())
                    .map_err(|e| format!("rsa pub: {e}"));
            }
            if let Ok(sk) = p256::SecretKey::from_pkcs8_der(der) {
                return sk
                    .public_key()
                    .to_public_key_der()
                    .map(|d| d.as_bytes().to_vec())
                    .map_err(|e| format!("p256 pub: {e}"));
            }
            if let Ok(sk) = p384::SecretKey::from_pkcs8_der(der) {
                return sk
                    .public_key()
                    .to_public_key_der()
                    .map(|d| d.as_bytes().to_vec())
                    .map_err(|e| format!("p384 pub: {e}"));
            }
            if let Ok(sk) = p521::SecretKey::from_pkcs8_der(der) {
                return sk
                    .public_key()
                    .to_public_key_der()
                    .map(|d| d.as_bytes().to_vec())
                    .map_err(|e| format!("p521 pub: {e}"));
            }
            if let Ok(signing) = ed25519_dalek::SigningKey::from_pkcs8_der(der) {
                return signing
                    .verifying_key()
                    .to_public_key_der()
                    .map(|d| d.as_bytes().to_vec())
                    .map_err(|e| format!("ed25519 pub: {e}"));
            }
            if let Ok(secret) = x25519_pkcs8_to_secret(der) {
                let public = x25519_dalek::PublicKey::from(&secret);
                return x25519_public_to_spki(&public);
            }
            Err("private-pkcs8 -> public-spki: unsupported key type".to_string())
        }
        _ => Err(format!(
            "unsupported conversion: {input_kind} -> {output_kind}"
        )),
    }
}

fn op_crypto_jwk_to_der<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let jwk_json = string_arg(scope, &args, 0);
    let want_kind = string_arg(scope, &args, 1);
    let jwk: serde_json::Value = match serde_json::from_str(&jwk_json) {
        Ok(j) => j,
        Err(e) => {
            throw_error(scope, &format!("jwk parse: {e}"));
            return;
        }
    };
    let result = jwk_to_der_impl(&jwk, &want_kind);
    match result {
        Ok(der) => {
            let arr = bytes_to_uint8array(scope, &der);
            rv.set(arr.into());
        }
        Err(e) => throw_error(scope, &e),
    }
}

fn jwk_to_der_impl(jwk: &serde_json::Value, want_kind: &str) -> Result<Vec<u8>, String> {
    use pkcs8::{EncodePrivateKey, EncodePublicKey};
    let kty = jwk["kty"]
        .as_str()
        .ok_or_else(|| "jwk missing kty".to_string())?;
    match kty {
        "RSA" => {
            let n_b64 = jwk["n"]
                .as_str()
                .ok_or_else(|| "RSA jwk missing n".to_string())?;
            let e_b64 = jwk["e"]
                .as_str()
                .ok_or_else(|| "RSA jwk missing e".to_string())?;
            let n_bytes = base64_url_decode(n_b64)?;
            let e_bytes = base64_url_decode(e_b64)?;
            let n = rsa::BigUint::from_bytes_be(&n_bytes);
            let e = rsa::BigUint::from_bytes_be(&e_bytes);
            if want_kind == "private-pkcs8" {
                let d_b64 = jwk["d"]
                    .as_str()
                    .ok_or_else(|| "RSA private jwk missing d".to_string())?;
                let d_bytes = base64_url_decode(d_b64)?;
                let d = rsa::BigUint::from_bytes_be(&d_bytes);
                let p_b64 = jwk["p"].as_str();
                let q_b64 = jwk["q"].as_str();
                let primes = if let (Some(p_str), Some(q_str)) = (p_b64, q_b64) {
                    let p_bytes = base64_url_decode(p_str)?;
                    let q_bytes = base64_url_decode(q_str)?;
                    let p = rsa::BigUint::from_bytes_be(&p_bytes);
                    let q = rsa::BigUint::from_bytes_be(&q_bytes);
                    vec![p, q]
                } else {
                    Vec::new()
                };
                let priv_key = if primes.is_empty() {
                    rsa::RsaPrivateKey::from_components(n, e, d, primes)
                        .map_err(|e| format!("RSA from components: {e}"))?
                } else {
                    rsa::RsaPrivateKey::from_components(n, e, d, primes)
                        .map_err(|e| format!("RSA from components: {e}"))?
                };
                priv_key
                    .to_pkcs8_der()
                    .map(|d| d.as_bytes().to_vec())
                    .map_err(|e| format!("RSA pkcs8: {e}"))
            } else {
                let pub_key = rsa::RsaPublicKey::new(n, e).map_err(|e| format!("RSA pub: {e}"))?;
                pub_key
                    .to_public_key_der()
                    .map(|d| d.as_bytes().to_vec())
                    .map_err(|e| format!("RSA spki: {e}"))
            }
        }
        "EC" => {
            let crv = jwk["crv"]
                .as_str()
                .ok_or_else(|| "EC jwk missing crv".to_string())?;
            let x_b64 = jwk["x"]
                .as_str()
                .ok_or_else(|| "EC jwk missing x".to_string())?;
            let y_b64 = jwk["y"]
                .as_str()
                .ok_or_else(|| "EC jwk missing y".to_string())?;
            let x_bytes = base64_url_decode(x_b64)?;
            let y_bytes = base64_url_decode(y_b64)?;
            match crv {
                "P-256" => {
                    if want_kind == "private-pkcs8" {
                        let d_b64 = jwk["d"]
                            .as_str()
                            .ok_or_else(|| "EC private jwk missing d".to_string())?;
                        let d_bytes = base64_url_decode(d_b64)?;
                        let sk = p256::SecretKey::from_slice(&d_bytes)
                            .map_err(|e| format!("P-256 secret: {e}"))?;
                        sk.to_pkcs8_der()
                            .map(|d| d.as_bytes().to_vec())
                            .map_err(|e| format!("P-256 pkcs8: {e}"))
                    } else {
                        let mut pt = vec![0x04u8];
                        pt.extend_from_slice(&x_bytes);
                        pt.extend_from_slice(&y_bytes);
                        let pk = p256::PublicKey::from_sec1_bytes(&pt)
                            .map_err(|e| format!("P-256 public: {e}"))?;
                        pk.to_public_key_der()
                            .map(|d| d.as_bytes().to_vec())
                            .map_err(|e| format!("P-256 spki: {e}"))
                    }
                }
                "P-384" => {
                    if want_kind == "private-pkcs8" {
                        let d_b64 = jwk["d"]
                            .as_str()
                            .ok_or_else(|| "EC private jwk missing d".to_string())?;
                        let d_bytes = base64_url_decode(d_b64)?;
                        let sk = p384::SecretKey::from_slice(&d_bytes)
                            .map_err(|e| format!("P-384 secret: {e}"))?;
                        sk.to_pkcs8_der()
                            .map(|d| d.as_bytes().to_vec())
                            .map_err(|e| format!("P-384 pkcs8: {e}"))
                    } else {
                        let mut pt = vec![0x04u8];
                        pt.extend_from_slice(&x_bytes);
                        pt.extend_from_slice(&y_bytes);
                        let pk = p384::PublicKey::from_sec1_bytes(&pt)
                            .map_err(|e| format!("P-384 public: {e}"))?;
                        pk.to_public_key_der()
                            .map(|d| d.as_bytes().to_vec())
                            .map_err(|e| format!("P-384 spki: {e}"))
                    }
                }
                "P-521" => {
                    if want_kind == "private-pkcs8" {
                        let d_b64 = jwk["d"]
                            .as_str()
                            .ok_or_else(|| "EC private jwk missing d".to_string())?;
                        let d_bytes = base64_url_decode(d_b64)?;
                        let sk = p521::SecretKey::from_slice(&d_bytes)
                            .map_err(|e| format!("P-521 secret: {e}"))?;
                        sk.to_pkcs8_der()
                            .map(|d| d.as_bytes().to_vec())
                            .map_err(|e| format!("P-521 pkcs8: {e}"))
                    } else {
                        let mut pt = vec![0x04u8];
                        pt.extend_from_slice(&x_bytes);
                        pt.extend_from_slice(&y_bytes);
                        let pk = p521::PublicKey::from_sec1_bytes(&pt)
                            .map_err(|e| format!("P-521 public: {e}"))?;
                        pk.to_public_key_der()
                            .map(|d| d.as_bytes().to_vec())
                            .map_err(|e| format!("P-521 spki: {e}"))
                    }
                }
                other => Err(format!("unsupported EC curve: {other}")),
            }
        }
        "OKP" => {
            let crv = jwk["crv"]
                .as_str()
                .ok_or_else(|| "OKP jwk missing crv".to_string())?;
            let x_b64 = jwk["x"]
                .as_str()
                .ok_or_else(|| "OKP jwk missing x".to_string())?;
            let x_bytes = base64_url_decode(x_b64)?;
            match crv {
                "Ed25519" => {
                    if want_kind == "private-pkcs8" {
                        let d_b64 = jwk["d"]
                            .as_str()
                            .ok_or_else(|| "OKP private jwk missing d".to_string())?;
                        let d_bytes = base64_url_decode(d_b64)?;
                        if d_bytes.len() != 32 {
                            return Err(format!(
                                "Ed25519 d must be 32 bytes, got {}",
                                d_bytes.len()
                            ));
                        }
                        let mut arr = [0u8; 32];
                        arr.copy_from_slice(&d_bytes);
                        let signing = ed25519_dalek::SigningKey::from_bytes(&arr);
                        signing
                            .to_pkcs8_der()
                            .map(|d| d.as_bytes().to_vec())
                            .map_err(|e| format!("Ed25519 pkcs8: {e}"))
                    } else {
                        if x_bytes.len() != 32 {
                            return Err(format!(
                                "Ed25519 x must be 32 bytes, got {}",
                                x_bytes.len()
                            ));
                        }
                        let mut arr = [0u8; 32];
                        arr.copy_from_slice(&x_bytes);
                        let verifying = ed25519_dalek::VerifyingKey::from_bytes(&arr)
                            .map_err(|e| format!("Ed25519 verifying: {e}"))?;
                        verifying
                            .to_public_key_der()
                            .map(|d| d.as_bytes().to_vec())
                            .map_err(|e| format!("Ed25519 spki: {e}"))
                    }
                }
                "X25519" => {
                    if want_kind == "private-pkcs8" {
                        let d_b64 = jwk["d"]
                            .as_str()
                            .ok_or_else(|| "OKP private jwk missing d".to_string())?;
                        let d_bytes = base64_url_decode(d_b64)?;
                        if d_bytes.len() != 32 {
                            return Err(format!(
                                "X25519 d must be 32 bytes, got {}",
                                d_bytes.len()
                            ));
                        }
                        let mut arr = [0u8; 32];
                        arr.copy_from_slice(&d_bytes);
                        let secret = x25519_dalek::StaticSecret::from(arr);
                        x25519_secret_to_pkcs8(&secret)
                    } else {
                        if x_bytes.len() != 32 {
                            return Err(format!(
                                "X25519 x must be 32 bytes, got {}",
                                x_bytes.len()
                            ));
                        }
                        let mut arr = [0u8; 32];
                        arr.copy_from_slice(&x_bytes);
                        let public = x25519_dalek::PublicKey::from(arr);
                        x25519_public_to_spki(&public)
                    }
                }
                other => Err(format!("unsupported OKP curve: {other}")),
            }
        }
        other => Err(format!("unsupported JWK kty: {other}")),
    }
}

fn base64_url_decode(s: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s)
        .map_err(|e| format!("base64url decode: {e}"))
}

fn base64_url_encode(b: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

fn op_crypto_der_to_jwk<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Some(der) = bytes_arg(scope, &args, 0) else {
        throw_error(scope, "der_to_jwk: der must be Uint8Array");
        return;
    };
    let kind = string_arg(scope, &args, 1);
    let result = der_to_jwk_impl(&der, &kind);
    match result {
        Ok(json_str) => {
            let s = v8::String::new(scope, &json_str).unwrap();
            rv.set(s.into());
        }
        Err(e) => throw_error(scope, &e),
    }
}

fn der_to_jwk_impl(der: &[u8], kind: &str) -> Result<String, String> {
    use p256::elliptic_curve::sec1::ToEncodedPoint;
    use pkcs8::{DecodePrivateKey, DecodePublicKey};
    use rsa::traits::{PrivateKeyParts, PublicKeyParts};
    match kind {
        "private-pkcs8" => {
            if let Ok(rsa_key) = rsa::RsaPrivateKey::from_pkcs8_der(der) {
                let n = base64_url_encode(&rsa_key.n().to_bytes_be());
                let e = base64_url_encode(&rsa_key.e().to_bytes_be());
                let d = base64_url_encode(&rsa_key.d().to_bytes_be());
                let primes = rsa_key.primes();
                let (p, q, dp, dq, qi) = if primes.len() >= 2 {
                    let p = base64_url_encode(&primes[0].to_bytes_be());
                    let q = base64_url_encode(&primes[1].to_bytes_be());
                    let dp_val = rsa_key.dp().ok_or("missing dp")?.to_bytes_be();
                    let dq_val = rsa_key.dq().ok_or("missing dq")?.to_bytes_be();
                    let qi_val = rsa_key.qinv().ok_or("missing qinv")?.to_bytes_be().1;
                    let dp = base64_url_encode(&dp_val);
                    let dq = base64_url_encode(&dq_val);
                    let qi = base64_url_encode(&qi_val);
                    (Some(p), Some(q), Some(dp), Some(dq), Some(qi))
                } else {
                    (None, None, None, None, None)
                };
                let mut jwk = serde_json::json!({
                    "kty": "RSA",
                    "n": n,
                    "e": e,
                    "d": d,
                });
                if let Some(p_val) = p {
                    jwk["p"] = serde_json::Value::String(p_val);
                    jwk["q"] = serde_json::Value::String(q.unwrap());
                    jwk["dp"] = serde_json::Value::String(dp.unwrap());
                    jwk["dq"] = serde_json::Value::String(dq.unwrap());
                    jwk["qi"] = serde_json::Value::String(qi.unwrap());
                }
                return Ok(jwk.to_string());
            }
            if let Ok(sk) = p256::SecretKey::from_pkcs8_der(der) {
                let d = base64_url_encode(&sk.to_bytes());
                let pk = sk.public_key();
                let pt = pk.to_encoded_point(false);
                let x = base64_url_encode(pt.x().ok_or("missing x")?);
                let y = base64_url_encode(pt.y().ok_or("missing y")?);
                let jwk = serde_json::json!({
                    "kty": "EC",
                    "crv": "P-256",
                    "x": x,
                    "y": y,
                    "d": d,
                });
                return Ok(jwk.to_string());
            }
            if let Ok(sk) = p384::SecretKey::from_pkcs8_der(der) {
                let d = base64_url_encode(&sk.to_bytes());
                let pk = sk.public_key();
                let pt = pk.to_encoded_point(false);
                let x = base64_url_encode(pt.x().ok_or("missing x")?);
                let y = base64_url_encode(pt.y().ok_or("missing y")?);
                let jwk = serde_json::json!({
                    "kty": "EC",
                    "crv": "P-384",
                    "x": x,
                    "y": y,
                    "d": d,
                });
                return Ok(jwk.to_string());
            }
            if let Ok(sk) = p521::SecretKey::from_pkcs8_der(der) {
                let d = base64_url_encode(&sk.to_bytes());
                let pk = sk.public_key();
                let pt = pk.to_encoded_point(false);
                let x = base64_url_encode(pt.x().ok_or("missing x")?);
                let y = base64_url_encode(pt.y().ok_or("missing y")?);
                let jwk = serde_json::json!({
                    "kty": "EC",
                    "crv": "P-521",
                    "x": x,
                    "y": y,
                    "d": d,
                });
                return Ok(jwk.to_string());
            }
            if let Ok(signing) = ed25519_dalek::SigningKey::from_pkcs8_der(der) {
                let d = base64_url_encode(&signing.to_bytes());
                let x = base64_url_encode(signing.verifying_key().as_bytes());
                let jwk = serde_json::json!({
                    "kty": "OKP",
                    "crv": "Ed25519",
                    "x": x,
                    "d": d,
                });
                return Ok(jwk.to_string());
            }
            if let Ok(secret) = x25519_pkcs8_to_secret(der) {
                let d = base64_url_encode(&secret.to_bytes());
                let public = x25519_dalek::PublicKey::from(&secret);
                let x = base64_url_encode(public.as_bytes());
                let jwk = serde_json::json!({
                    "kty": "OKP",
                    "crv": "X25519",
                    "x": x,
                    "d": d,
                });
                return Ok(jwk.to_string());
            }
            Err("private-pkcs8: unsupported key type for JWK".to_string())
        }
        "public-spki" => {
            if let Ok(rsa_key) = rsa::RsaPublicKey::from_public_key_der(der) {
                let n = base64_url_encode(&rsa_key.n().to_bytes_be());
                let e = base64_url_encode(&rsa_key.e().to_bytes_be());
                let jwk = serde_json::json!({
                    "kty": "RSA",
                    "n": n,
                    "e": e,
                });
                return Ok(jwk.to_string());
            }
            if let Ok(pk) = p256::PublicKey::from_public_key_der(der) {
                let pt = pk.to_encoded_point(false);
                let x = base64_url_encode(pt.x().ok_or("missing x")?);
                let y = base64_url_encode(pt.y().ok_or("missing y")?);
                let jwk = serde_json::json!({
                    "kty": "EC",
                    "crv": "P-256",
                    "x": x,
                    "y": y,
                });
                return Ok(jwk.to_string());
            }
            if let Ok(pk) = p384::PublicKey::from_public_key_der(der) {
                let pt = pk.to_encoded_point(false);
                let x = base64_url_encode(pt.x().ok_or("missing x")?);
                let y = base64_url_encode(pt.y().ok_or("missing y")?);
                let jwk = serde_json::json!({
                    "kty": "EC",
                    "crv": "P-384",
                    "x": x,
                    "y": y,
                });
                return Ok(jwk.to_string());
            }
            if let Ok(pk) = p521::PublicKey::from_public_key_der(der) {
                let pt = pk.to_encoded_point(false);
                let x = base64_url_encode(pt.x().ok_or("missing x")?);
                let y = base64_url_encode(pt.y().ok_or("missing y")?);
                let jwk = serde_json::json!({
                    "kty": "EC",
                    "crv": "P-521",
                    "x": x,
                    "y": y,
                });
                return Ok(jwk.to_string());
            }
            if let Ok(verifying) = ed25519_dalek::VerifyingKey::from_public_key_der(der) {
                let x = base64_url_encode(verifying.as_bytes());
                let jwk = serde_json::json!({
                    "kty": "OKP",
                    "crv": "Ed25519",
                    "x": x,
                });
                return Ok(jwk.to_string());
            }
            if let Ok(public) = x25519_spki_to_public(der) {
                let x = base64_url_encode(public.as_bytes());
                let jwk = serde_json::json!({
                    "kty": "OKP",
                    "crv": "X25519",
                    "x": x,
                });
                return Ok(jwk.to_string());
            }
            Err("public-spki: unsupported key type for JWK".to_string())
        }
        other => Err(format!("unsupported kind for JWK: {other}")),
    }
}

fn op_crypto_rsa_encrypt<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Some(spki_der) = bytes_arg(scope, &args, 0) else {
        throw_error(scope, "rsa_encrypt: spki_der must be Uint8Array");
        return;
    };
    let Some(plaintext) = bytes_arg(scope, &args, 1) else {
        throw_error(scope, "rsa_encrypt: plaintext must be Uint8Array");
        return;
    };
    let padding = string_arg(scope, &args, 2);
    let oaep_hash = string_arg(scope, &args, 3);
    let oaep_label = bytes_arg(scope, &args, 4);
    let result = rsa_encrypt_impl(
        &spki_der,
        &plaintext,
        &padding,
        &oaep_hash,
        oaep_label.as_deref(),
    );
    match result {
        Ok(ct) => {
            let arr = bytes_to_uint8array(scope, &ct);
            rv.set(arr.into());
        }
        Err(e) => throw_error(scope, &e),
    }
}

fn rsa_encrypt_impl(
    spki_der: &[u8],
    plaintext: &[u8],
    padding: &str,
    oaep_hash: &str,
    oaep_label: Option<&[u8]>,
) -> Result<Vec<u8>, String> {
    use pkcs8::DecodePublicKey;
    use rsa::{Oaep, Pkcs1v15Encrypt};
    let pub_key =
        rsa::RsaPublicKey::from_public_key_der(spki_der).map_err(|e| format!("rsa pub: {e}"))?;
    let mut rng = rand_core::OsRng;
    match padding {
        "oaep" => {
            let label_bytes = oaep_label.unwrap_or(&[]);
            let label_str = if label_bytes.is_empty() {
                String::new()
            } else {
                std::str::from_utf8(label_bytes)
                    .map_err(|_| "rsa oaep: label must be valid UTF-8".to_string())?
                    .to_string()
            };
            let label = label_str.as_str();
            match oaep_hash {
                "sha1" => {
                    let padding = if label.is_empty() {
                        Oaep::new::<sha1::Sha1>()
                    } else {
                        Oaep::new_with_label::<sha1::Sha1, _>(label)
                    };
                    pub_key
                        .encrypt(&mut rng, padding, plaintext)
                        .map_err(|e| format!("rsa oaep encrypt: {e}"))
                }
                "sha256" => {
                    let padding = if label.is_empty() {
                        Oaep::new::<sha2::Sha256>()
                    } else {
                        Oaep::new_with_label::<sha2::Sha256, _>(label)
                    };
                    pub_key
                        .encrypt(&mut rng, padding, plaintext)
                        .map_err(|e| format!("rsa oaep encrypt: {e}"))
                }
                "sha384" => {
                    let padding = if label.is_empty() {
                        Oaep::new::<sha2::Sha384>()
                    } else {
                        Oaep::new_with_label::<sha2::Sha384, _>(label)
                    };
                    pub_key
                        .encrypt(&mut rng, padding, plaintext)
                        .map_err(|e| format!("rsa oaep encrypt: {e}"))
                }
                "sha512" => {
                    let padding = if label.is_empty() {
                        Oaep::new::<sha2::Sha512>()
                    } else {
                        Oaep::new_with_label::<sha2::Sha512, _>(label)
                    };
                    pub_key
                        .encrypt(&mut rng, padding, plaintext)
                        .map_err(|e| format!("rsa oaep encrypt: {e}"))
                }
                other => Err(format!("unsupported oaep hash: {other}")),
            }
        }
        "pkcs1" => {
            let padding = Pkcs1v15Encrypt;
            pub_key
                .encrypt(&mut rng, padding, plaintext)
                .map_err(|e| format!("rsa pkcs1 encrypt: {e}"))
        }
        other => Err(format!("unsupported rsa padding: {other}")),
    }
}

fn op_crypto_rsa_decrypt<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Some(pkcs8_der) = bytes_arg(scope, &args, 0) else {
        throw_error(scope, "rsa_decrypt: pkcs8_der must be Uint8Array");
        return;
    };
    let Some(ciphertext) = bytes_arg(scope, &args, 1) else {
        throw_error(scope, "rsa_decrypt: ciphertext must be Uint8Array");
        return;
    };
    let padding = string_arg(scope, &args, 2);
    let oaep_hash = string_arg(scope, &args, 3);
    let oaep_label = bytes_arg(scope, &args, 4);
    let result = rsa_decrypt_impl(
        &pkcs8_der,
        &ciphertext,
        &padding,
        &oaep_hash,
        oaep_label.as_deref(),
    );
    match result {
        Ok(pt) => {
            let arr = bytes_to_uint8array(scope, &pt);
            rv.set(arr.into());
        }
        Err(e) => throw_error(scope, &e),
    }
}

fn rsa_decrypt_impl(
    pkcs8_der: &[u8],
    ciphertext: &[u8],
    padding: &str,
    oaep_hash: &str,
    oaep_label: Option<&[u8]>,
) -> Result<Vec<u8>, String> {
    use pkcs8::DecodePrivateKey;
    use rsa::{Oaep, Pkcs1v15Encrypt};
    let priv_key =
        rsa::RsaPrivateKey::from_pkcs8_der(pkcs8_der).map_err(|e| format!("rsa priv: {e}"))?;
    match padding {
        "oaep" => {
            let label_bytes = oaep_label.unwrap_or(&[]);
            let label_str = if label_bytes.is_empty() {
                String::new()
            } else {
                std::str::from_utf8(label_bytes)
                    .map_err(|_| "rsa oaep: label must be valid UTF-8".to_string())?
                    .to_string()
            };
            let label = label_str.as_str();
            match oaep_hash {
                "sha1" => {
                    let padding = if label.is_empty() {
                        Oaep::new::<sha1::Sha1>()
                    } else {
                        Oaep::new_with_label::<sha1::Sha1, _>(label)
                    };
                    priv_key
                        .decrypt(padding, ciphertext)
                        .map_err(|e| format!("rsa oaep decrypt: {e}"))
                }
                "sha256" => {
                    let padding = if label.is_empty() {
                        Oaep::new::<sha2::Sha256>()
                    } else {
                        Oaep::new_with_label::<sha2::Sha256, _>(label)
                    };
                    priv_key
                        .decrypt(padding, ciphertext)
                        .map_err(|e| format!("rsa oaep decrypt: {e}"))
                }
                "sha384" => {
                    let padding = if label.is_empty() {
                        Oaep::new::<sha2::Sha384>()
                    } else {
                        Oaep::new_with_label::<sha2::Sha384, _>(label)
                    };
                    priv_key
                        .decrypt(padding, ciphertext)
                        .map_err(|e| format!("rsa oaep decrypt: {e}"))
                }
                "sha512" => {
                    let padding = if label.is_empty() {
                        Oaep::new::<sha2::Sha512>()
                    } else {
                        Oaep::new_with_label::<sha2::Sha512, _>(label)
                    };
                    priv_key
                        .decrypt(padding, ciphertext)
                        .map_err(|e| format!("rsa oaep decrypt: {e}"))
                }
                other => Err(format!("unsupported oaep hash: {other}")),
            }
        }
        "pkcs1" => {
            let padding = Pkcs1v15Encrypt;
            priv_key
                .decrypt(padding, ciphertext)
                .map_err(|e| format!("rsa pkcs1 decrypt: {e}"))
        }
        other => Err(format!("unsupported rsa padding: {other}")),
    }
}

fn op_crypto_sign_der<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let algo = string_arg(scope, &args, 0);
    let kind = string_arg(scope, &args, 1);
    let Some(der) = bytes_arg(scope, &args, 2) else {
        throw_error(scope, "sign_der: der must be Uint8Array");
        return;
    };
    let Some(data) = bytes_arg(scope, &args, 3) else {
        throw_error(scope, "sign_der: data must be Uint8Array");
        return;
    };
    let format = if args.length() >= 5 {
        string_arg(scope, &args, 4)
    } else {
        "der".to_string()
    };
    let result = sign_der_impl(&algo, &kind, &der, &data, &format);
    match result {
        Ok(sig) => {
            let arr = bytes_to_uint8array(scope, &sig);
            rv.set(arr.into());
        }
        Err(e) => throw_error(scope, &e),
    }
}

fn sign_der_impl(
    algo: &str,
    kind: &str,
    der: &[u8],
    data: &[u8],
    format: &str,
) -> Result<Vec<u8>, String> {
    if kind != "private-pkcs8" {
        return Err(format!("sign_der: unsupported kind {kind}"));
    }
    match algo {
        "rsa-sha256" => rsa_sign_der::<sha2::Sha256>(der, data),
        "rsa-sha384" => rsa_sign_der::<sha2::Sha384>(der, data),
        "rsa-sha512" => rsa_sign_der::<sha2::Sha512>(der, data),
        "rsa-pss-sha256" => rsa_pss_sign_der::<sha2::Sha256>(der, data),
        "rsa-pss-sha384" => rsa_pss_sign_der::<sha2::Sha384>(der, data),
        "rsa-pss-sha512" => rsa_pss_sign_der::<sha2::Sha512>(der, data),
        "ecdsa-p256-sha256" => ecdsa_sign_der_p256(der, data, format),
        "ecdsa-p384-sha384" => ecdsa_sign_der_p384(der, data, format),
        "ecdsa-p521-sha512" => ecdsa_sign_der_p521(der, data, format),
        "ed25519" => ed25519_sign_der(der, data),
        other => Err(format!("unsupported sign_der algorithm: {other}")),
    }
}

fn rsa_sign_der<D>(der: &[u8], data: &[u8]) -> Result<Vec<u8>, String>
where
    D: digest::Digest + digest::const_oid::AssociatedOid,
{
    use pkcs8::DecodePrivateKey;
    use rsa::pkcs1v15::SigningKey;
    use rsa::signature::{SignatureEncoding, Signer};
    let priv_key = rsa::RsaPrivateKey::from_pkcs8_der(der).map_err(|e| format!("rsa priv: {e}"))?;
    let signing_key = SigningKey::<D>::new(priv_key);
    let sig = signing_key.sign(data);
    Ok(sig.to_bytes().into_vec())
}

fn rsa_pss_sign_der<D>(der: &[u8], data: &[u8]) -> Result<Vec<u8>, String>
where
    D: digest::Digest + digest::const_oid::AssociatedOid + digest::FixedOutputReset,
{
    use pkcs8::DecodePrivateKey;
    use rsa::pss::SigningKey;
    use rsa::signature::{RandomizedSigner, SignatureEncoding};
    let priv_key = rsa::RsaPrivateKey::from_pkcs8_der(der).map_err(|e| format!("rsa priv: {e}"))?;
    let signing_key = SigningKey::<D>::new(priv_key);
    let mut rng = rand_core::OsRng;
    let sig = signing_key.sign_with_rng(&mut rng, data);
    Ok(sig.to_bytes().into_vec())
}

fn ecdsa_sign_der_p256(der: &[u8], data: &[u8], format: &str) -> Result<Vec<u8>, String> {
    use p256::ecdsa::signature::Signer;
    use p256::ecdsa::{Signature, SigningKey};
    use pkcs8::DecodePrivateKey;
    let signing = SigningKey::from_pkcs8_der(der).map_err(|e| format!("p256 priv: {e}"))?;
    let sig: Signature = signing.sign(data);
    match format {
        "der" => Ok(sig.to_der().as_bytes().to_vec()),
        "ieee-p1363" => Ok(sig.to_bytes().to_vec()),
        other => Err(format!("unsupported ecdsa format: {other}")),
    }
}

fn ecdsa_sign_der_p384(der: &[u8], data: &[u8], format: &str) -> Result<Vec<u8>, String> {
    use p384::ecdsa::signature::Signer;
    use p384::ecdsa::{Signature, SigningKey};
    use pkcs8::DecodePrivateKey;
    let signing = SigningKey::from_pkcs8_der(der).map_err(|e| format!("p384 priv: {e}"))?;
    let sig: Signature = signing.sign(data);
    match format {
        "der" => Ok(sig.to_der().as_bytes().to_vec()),
        "ieee-p1363" => Ok(sig.to_bytes().to_vec()),
        other => Err(format!("unsupported ecdsa format: {other}")),
    }
}

fn ecdsa_sign_der_p521(der: &[u8], data: &[u8], format: &str) -> Result<Vec<u8>, String> {
    use p521::ecdsa::signature::Signer;
    use p521::ecdsa::{Signature, SigningKey};
    use pkcs8::DecodePrivateKey;
    let secret = p521::SecretKey::from_pkcs8_der(der).map_err(|e| format!("p521 priv: {e}"))?;
    let signing =
        SigningKey::from_slice(&secret.to_bytes()).map_err(|e| format!("p521 signing key: {e}"))?;
    let sig: Signature = signing.sign(data);
    match format {
        "der" => Ok(sig.to_der().as_bytes().to_vec()),
        "ieee-p1363" => Ok(sig.to_bytes().to_vec()),
        other => Err(format!("unsupported ecdsa format: {other}")),
    }
}

fn ed25519_sign_der(der: &[u8], data: &[u8]) -> Result<Vec<u8>, String> {
    use ed25519_dalek::Signer;
    use pkcs8::DecodePrivateKey;
    let signing =
        ed25519_dalek::SigningKey::from_pkcs8_der(der).map_err(|e| format!("ed25519 priv: {e}"))?;
    let sig = signing.sign(data);
    Ok(sig.to_bytes().to_vec())
}

fn op_crypto_verify_der<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let algo = string_arg(scope, &args, 0);
    let kind = string_arg(scope, &args, 1);
    let Some(der) = bytes_arg(scope, &args, 2) else {
        throw_error(scope, "verify_der: der must be Uint8Array");
        return;
    };
    let Some(data) = bytes_arg(scope, &args, 3) else {
        throw_error(scope, "verify_der: data must be Uint8Array");
        return;
    };
    let Some(sig) = bytes_arg(scope, &args, 4) else {
        throw_error(scope, "verify_der: signature must be Uint8Array");
        return;
    };
    let format = if args.length() >= 6 {
        string_arg(scope, &args, 5)
    } else {
        "auto".to_string()
    };
    let result = verify_der_impl(&algo, &kind, &der, &data, &sig, &format);
    match result {
        Ok(ok) => rv.set(v8::Boolean::new(scope, ok).into()),
        Err(e) => throw_error(scope, &e),
    }
}

fn verify_der_impl(
    algo: &str,
    kind: &str,
    der: &[u8],
    data: &[u8],
    sig: &[u8],
    format: &str,
) -> Result<bool, String> {
    if kind != "public-spki" {
        return Err(format!("verify_der: unsupported kind {kind}"));
    }
    match algo {
        "rsa-sha256" => rsa_verify_der::<sha2::Sha256>(der, data, sig),
        "rsa-sha384" => rsa_verify_der::<sha2::Sha384>(der, data, sig),
        "rsa-sha512" => rsa_verify_der::<sha2::Sha512>(der, data, sig),
        "rsa-pss-sha256" => rsa_pss_verify_der::<sha2::Sha256>(der, data, sig),
        "rsa-pss-sha384" => rsa_pss_verify_der::<sha2::Sha384>(der, data, sig),
        "rsa-pss-sha512" => rsa_pss_verify_der::<sha2::Sha512>(der, data, sig),
        "ecdsa-p256-sha256" => ecdsa_verify_der_p256(der, data, sig, format),
        "ecdsa-p384-sha384" => ecdsa_verify_der_p384(der, data, sig, format),
        "ecdsa-p521-sha512" => ecdsa_verify_der_p521(der, data, sig, format),
        "ed25519" => ed25519_verify_der(der, data, sig),
        other => Err(format!("unsupported verify_der algorithm: {other}")),
    }
}

fn rsa_verify_der<D>(der: &[u8], data: &[u8], sig: &[u8]) -> Result<bool, String>
where
    D: digest::Digest + digest::const_oid::AssociatedOid,
{
    use pkcs8::DecodePublicKey;
    use rsa::pkcs1v15::{Signature, VerifyingKey};
    use rsa::signature::Verifier;
    let pub_key =
        rsa::RsaPublicKey::from_public_key_der(der).map_err(|e| format!("rsa pub: {e}"))?;
    let verifying = VerifyingKey::<D>::new(pub_key);
    let signature = Signature::try_from(sig).map_err(|e| format!("rsa sig: {e}"))?;
    Ok(verifying.verify(data, &signature).is_ok())
}

fn rsa_pss_verify_der<D>(der: &[u8], data: &[u8], sig: &[u8]) -> Result<bool, String>
where
    D: digest::Digest + digest::const_oid::AssociatedOid + digest::FixedOutputReset,
{
    use pkcs8::DecodePublicKey;
    use rsa::pss::{Signature, VerifyingKey};
    use rsa::signature::Verifier;
    let pub_key =
        rsa::RsaPublicKey::from_public_key_der(der).map_err(|e| format!("rsa pub: {e}"))?;
    let verifying = VerifyingKey::<D>::new(pub_key);
    let signature = Signature::try_from(sig).map_err(|e| format!("rsa sig: {e}"))?;
    Ok(verifying.verify(data, &signature).is_ok())
}

fn ecdsa_verify_der_p256(
    der: &[u8],
    data: &[u8],
    sig: &[u8],
    format: &str,
) -> Result<bool, String> {
    use p256::ecdsa::signature::Verifier;
    use p256::ecdsa::{Signature, VerifyingKey};
    use pkcs8::DecodePublicKey;
    let verifying = VerifyingKey::from_public_key_der(der).map_err(|e| format!("p256 pub: {e}"))?;
    match format {
        "auto" => {
            if let Ok(signature) = Signature::from_der(sig)
                && verifying.verify(data, &signature).is_ok()
            {
                return Ok(true);
            }
            if let Ok(signature) = Signature::try_from(sig)
                && verifying.verify(data, &signature).is_ok()
            {
                return Ok(true);
            }
            Ok(false)
        }
        "der" => {
            let signature = Signature::from_der(sig).map_err(|e| format!("p256 sig der: {e}"))?;
            Ok(verifying.verify(data, &signature).is_ok())
        }
        "ieee-p1363" => {
            let signature = Signature::try_from(sig).map_err(|e| format!("p256 sig ieee: {e}"))?;
            Ok(verifying.verify(data, &signature).is_ok())
        }
        other => Err(format!("unsupported ecdsa format: {other}")),
    }
}

fn ecdsa_verify_der_p384(
    der: &[u8],
    data: &[u8],
    sig: &[u8],
    format: &str,
) -> Result<bool, String> {
    use p384::ecdsa::signature::Verifier;
    use p384::ecdsa::{Signature, VerifyingKey};
    use pkcs8::DecodePublicKey;
    let verifying = VerifyingKey::from_public_key_der(der).map_err(|e| format!("p384 pub: {e}"))?;
    match format {
        "auto" => {
            if let Ok(signature) = Signature::from_der(sig)
                && verifying.verify(data, &signature).is_ok()
            {
                return Ok(true);
            }
            if let Ok(signature) = Signature::try_from(sig)
                && verifying.verify(data, &signature).is_ok()
            {
                return Ok(true);
            }
            Ok(false)
        }
        "der" => {
            let signature = Signature::from_der(sig).map_err(|e| format!("p384 sig der: {e}"))?;
            Ok(verifying.verify(data, &signature).is_ok())
        }
        "ieee-p1363" => {
            let signature = Signature::try_from(sig).map_err(|e| format!("p384 sig ieee: {e}"))?;
            Ok(verifying.verify(data, &signature).is_ok())
        }
        other => Err(format!("unsupported ecdsa format: {other}")),
    }
}

fn ecdsa_verify_der_p521(
    der: &[u8],
    data: &[u8],
    sig: &[u8],
    format: &str,
) -> Result<bool, String> {
    use p521::ecdsa::signature::Verifier;
    use p521::ecdsa::{Signature, VerifyingKey};
    use pkcs8::DecodePublicKey;
    let public = p521::PublicKey::from_public_key_der(der).map_err(|e| format!("p521 pub: {e}"))?;
    let verifying = VerifyingKey::from_sec1_bytes(&public.to_sec1_bytes())
        .map_err(|e| format!("p521 verifying key: {e}"))?;
    match format {
        "auto" => {
            if let Ok(signature) = Signature::from_der(sig)
                && verifying.verify(data, &signature).is_ok()
            {
                return Ok(true);
            }
            if let Ok(signature) = Signature::try_from(sig)
                && verifying.verify(data, &signature).is_ok()
            {
                return Ok(true);
            }
            Ok(false)
        }
        "der" => {
            let signature = Signature::from_der(sig).map_err(|e| format!("p521 sig der: {e}"))?;
            Ok(verifying.verify(data, &signature).is_ok())
        }
        "ieee-p1363" => {
            let signature = Signature::try_from(sig).map_err(|e| format!("p521 sig ieee: {e}"))?;
            Ok(verifying.verify(data, &signature).is_ok())
        }
        other => Err(format!("unsupported ecdsa format: {other}")),
    }
}

fn ed25519_verify_der(der: &[u8], data: &[u8], sig: &[u8]) -> Result<bool, String> {
    use ed25519_dalek::{Signature, Verifier};
    use pkcs8::DecodePublicKey;
    let verifying = ed25519_dalek::VerifyingKey::from_public_key_der(der)
        .map_err(|e| format!("ed25519 pub: {e}"))?;
    if sig.len() != 64 {
        return Err(format!("ed25519 sig must be 64 bytes, got {}", sig.len()));
    }
    let mut sig_arr = [0u8; 64];
    sig_arr.copy_from_slice(sig);
    let signature = Signature::from_bytes(&sig_arr);
    Ok(verifying.verify(data, &signature).is_ok())
}

fn op_crypto_ecdh_derive<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let curve = string_arg(scope, &args, 0);
    let Some(priv_der) = bytes_arg(scope, &args, 1) else {
        throw_error(scope, "ecdh_derive: priv_der must be Uint8Array");
        return;
    };
    let Some(pub_der) = bytes_arg(scope, &args, 2) else {
        throw_error(scope, "ecdh_derive: pub_der must be Uint8Array");
        return;
    };
    let result = ecdh_derive_impl(&curve, &priv_der, &pub_der);
    match result {
        Ok(secret) => {
            let arr = bytes_to_uint8array(scope, &secret);
            rv.set(arr.into());
        }
        Err(e) => throw_error(scope, &e),
    }
}

fn ecdh_derive_impl(curve: &str, priv_der: &[u8], pub_der: &[u8]) -> Result<Vec<u8>, String> {
    use p256::elliptic_curve::ecdh::diffie_hellman;
    use pkcs8::{DecodePrivateKey, DecodePublicKey};
    match curve {
        "P-256" | "prime256v1" => {
            let priv_key =
                p256::SecretKey::from_pkcs8_der(priv_der).map_err(|e| format!("p256 priv: {e}"))?;
            let pub_key = p256::PublicKey::from_public_key_der(pub_der)
                .map_err(|e| format!("p256 pub: {e}"))?;
            let shared = diffie_hellman(priv_key.to_nonzero_scalar(), pub_key.as_affine());
            Ok(shared.raw_secret_bytes().to_vec())
        }
        "P-384" | "secp384r1" => {
            let priv_key =
                p384::SecretKey::from_pkcs8_der(priv_der).map_err(|e| format!("p384 priv: {e}"))?;
            let pub_key = p384::PublicKey::from_public_key_der(pub_der)
                .map_err(|e| format!("p384 pub: {e}"))?;
            let shared = diffie_hellman(priv_key.to_nonzero_scalar(), pub_key.as_affine());
            Ok(shared.raw_secret_bytes().to_vec())
        }
        "P-521" | "secp521r1" => {
            let priv_key =
                p521::SecretKey::from_pkcs8_der(priv_der).map_err(|e| format!("p521 priv: {e}"))?;
            let pub_key = p521::PublicKey::from_public_key_der(pub_der)
                .map_err(|e| format!("p521 pub: {e}"))?;
            let shared = diffie_hellman(priv_key.to_nonzero_scalar(), pub_key.as_affine());
            Ok(shared.raw_secret_bytes().to_vec())
        }
        other => Err(format!("unsupported ecdh curve: {other}")),
    }
}

fn op_crypto_x25519_derive<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Some(priv_der) = bytes_arg(scope, &args, 0) else {
        throw_error(scope, "x25519_derive: priv_der must be Uint8Array");
        return;
    };
    let Some(pub_der) = bytes_arg(scope, &args, 1) else {
        throw_error(scope, "x25519_derive: pub_der must be Uint8Array");
        return;
    };
    let result = x25519_derive_impl(&priv_der, &pub_der);
    match result {
        Ok(secret) => {
            let arr = bytes_to_uint8array(scope, &secret);
            rv.set(arr.into());
        }
        Err(e) => throw_error(scope, &e),
    }
}

fn x25519_derive_impl(priv_der: &[u8], pub_der: &[u8]) -> Result<Vec<u8>, String> {
    let secret = x25519_pkcs8_to_secret(priv_der)?;
    let public = x25519_spki_to_public(pub_der)?;
    let shared = secret.diffie_hellman(&public);
    Ok(shared.as_bytes().to_vec())
}

fn op_crypto_ecdh_generate<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let curve = string_arg(scope, &args, 0);
    let result = ecdh_generate_impl(&curve);
    match result {
        Ok((pub_der, priv_der, pub_raw, priv_raw)) => {
            let obj = v8::Object::new(scope);
            let pub_key = v8::String::new(scope, "publicKey").unwrap();
            let pub_arr = bytes_to_uint8array(scope, &pub_der);
            obj.set(scope, pub_key.into(), pub_arr.into());
            let priv_key = v8::String::new(scope, "privateKey").unwrap();
            let priv_arr = bytes_to_uint8array(scope, &priv_der);
            obj.set(scope, priv_key.into(), priv_arr.into());
            let pub_raw_key = v8::String::new(scope, "publicRaw").unwrap();
            let pub_raw_arr = bytes_to_uint8array(scope, &pub_raw);
            obj.set(scope, pub_raw_key.into(), pub_raw_arr.into());
            let priv_raw_key = v8::String::new(scope, "privateRaw").unwrap();
            let priv_raw_arr = bytes_to_uint8array(scope, &priv_raw);
            obj.set(scope, priv_raw_key.into(), priv_raw_arr.into());
            rv.set(obj.into());
        }
        Err(e) => throw_error(scope, &e),
    }
}

type EcdhKeyPair = (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>);

fn ecdh_generate_impl(curve: &str) -> Result<EcdhKeyPair, String> {
    use p256::elliptic_curve::sec1::ToEncodedPoint;
    use pkcs8::{EncodePrivateKey, EncodePublicKey};
    use rand_core::OsRng;
    match curve {
        "P-256" | "prime256v1" => {
            let secret = p256::SecretKey::random(&mut OsRng);
            let public = secret.public_key();
            let priv_der = secret
                .to_pkcs8_der()
                .map_err(|e| format!("p256 priv: {e}"))?
                .as_bytes()
                .to_vec();
            let pub_der = public
                .to_public_key_der()
                .map_err(|e| format!("p256 pub: {e}"))?
                .as_bytes()
                .to_vec();
            let priv_raw = secret.to_bytes().to_vec();
            let pub_raw = public.to_encoded_point(false).as_bytes().to_vec();
            Ok((pub_der, priv_der, pub_raw, priv_raw))
        }
        "P-384" | "secp384r1" => {
            let secret = p384::SecretKey::random(&mut OsRng);
            let public = secret.public_key();
            let priv_der = secret
                .to_pkcs8_der()
                .map_err(|e| format!("p384 priv: {e}"))?
                .as_bytes()
                .to_vec();
            let pub_der = public
                .to_public_key_der()
                .map_err(|e| format!("p384 pub: {e}"))?
                .as_bytes()
                .to_vec();
            let priv_raw = secret.to_bytes().to_vec();
            let pub_raw = public.to_encoded_point(false).as_bytes().to_vec();
            Ok((pub_der, priv_der, pub_raw, priv_raw))
        }
        "P-521" | "secp521r1" => {
            let secret = p521::SecretKey::random(&mut OsRng);
            let public = secret.public_key();
            let priv_der = secret
                .to_pkcs8_der()
                .map_err(|e| format!("p521 priv: {e}"))?
                .as_bytes()
                .to_vec();
            let pub_der = public
                .to_public_key_der()
                .map_err(|e| format!("p521 pub: {e}"))?
                .as_bytes()
                .to_vec();
            let priv_raw = secret.to_bytes().to_vec();
            let pub_raw = public.to_encoded_point(false).as_bytes().to_vec();
            Ok((pub_der, priv_der, pub_raw, priv_raw))
        }
        other => Err(format!("unsupported ecdh curve: {other}")),
    }
}

fn op_crypto_ecdh_from_raw<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let curve = string_arg(scope, &args, 0);
    let Some(priv_raw) = bytes_arg(scope, &args, 1) else {
        throw_error(scope, "ecdh_from_raw: priv_raw must be Uint8Array");
        return;
    };
    let result = ecdh_from_raw_impl(&curve, &priv_raw);
    match result {
        Ok((pub_der, priv_der, pub_raw, priv_raw)) => {
            let obj = v8::Object::new(scope);
            let pub_key = v8::String::new(scope, "publicKey").unwrap();
            let pub_arr = bytes_to_uint8array(scope, &pub_der);
            obj.set(scope, pub_key.into(), pub_arr.into());
            let priv_key = v8::String::new(scope, "privateKey").unwrap();
            let priv_arr = bytes_to_uint8array(scope, &priv_der);
            obj.set(scope, priv_key.into(), priv_arr.into());
            let pub_raw_key = v8::String::new(scope, "publicRaw").unwrap();
            let pub_raw_arr = bytes_to_uint8array(scope, &pub_raw);
            obj.set(scope, pub_raw_key.into(), pub_raw_arr.into());
            let priv_raw_key = v8::String::new(scope, "privateRaw").unwrap();
            let priv_raw_arr = bytes_to_uint8array(scope, &priv_raw);
            obj.set(scope, priv_raw_key.into(), priv_raw_arr.into());
            rv.set(obj.into());
        }
        Err(e) => throw_error(scope, &e),
    }
}

fn ecdh_from_raw_impl(curve: &str, priv_raw: &[u8]) -> Result<EcdhKeyPair, String> {
    use p256::elliptic_curve::sec1::ToEncodedPoint;
    use pkcs8::{EncodePrivateKey, EncodePublicKey};
    match curve {
        "P-256" | "prime256v1" => {
            let secret =
                p256::SecretKey::from_slice(priv_raw).map_err(|e| format!("p256 from raw: {e}"))?;
            let public = secret.public_key();
            let priv_der = secret
                .to_pkcs8_der()
                .map_err(|e| format!("p256 priv: {e}"))?
                .as_bytes()
                .to_vec();
            let pub_der = public
                .to_public_key_der()
                .map_err(|e| format!("p256 pub: {e}"))?
                .as_bytes()
                .to_vec();
            let priv_raw_out = secret.to_bytes().to_vec();
            let pub_raw = public.to_encoded_point(false).as_bytes().to_vec();
            Ok((pub_der, priv_der, pub_raw, priv_raw_out))
        }
        "P-384" | "secp384r1" => {
            let secret =
                p384::SecretKey::from_slice(priv_raw).map_err(|e| format!("p384 from raw: {e}"))?;
            let public = secret.public_key();
            let priv_der = secret
                .to_pkcs8_der()
                .map_err(|e| format!("p384 priv: {e}"))?
                .as_bytes()
                .to_vec();
            let pub_der = public
                .to_public_key_der()
                .map_err(|e| format!("p384 pub: {e}"))?
                .as_bytes()
                .to_vec();
            let priv_raw_out = secret.to_bytes().to_vec();
            let pub_raw = public.to_encoded_point(false).as_bytes().to_vec();
            Ok((pub_der, priv_der, pub_raw, priv_raw_out))
        }
        "P-521" | "secp521r1" => {
            let secret =
                p521::SecretKey::from_slice(priv_raw).map_err(|e| format!("p521 from raw: {e}"))?;
            let public = secret.public_key();
            let priv_der = secret
                .to_pkcs8_der()
                .map_err(|e| format!("p521 priv: {e}"))?
                .as_bytes()
                .to_vec();
            let pub_der = public
                .to_public_key_der()
                .map_err(|e| format!("p521 pub: {e}"))?
                .as_bytes()
                .to_vec();
            let priv_raw_out = secret.to_bytes().to_vec();
            let pub_raw = public.to_encoded_point(false).as_bytes().to_vec();
            Ok((pub_der, priv_der, pub_raw, priv_raw_out))
        }
        other => Err(format!("unsupported ecdh curve: {other}")),
    }
}

fn op_crypto_ecdh_compute_raw<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let curve = string_arg(scope, &args, 0);
    let Some(priv_raw) = bytes_arg(scope, &args, 1) else {
        throw_error(scope, "ecdh_compute_raw: priv_raw must be Uint8Array");
        return;
    };
    let Some(pub_raw) = bytes_arg(scope, &args, 2) else {
        throw_error(scope, "ecdh_compute_raw: pub_raw must be Uint8Array");
        return;
    };
    let result = ecdh_compute_raw_impl(&curve, &priv_raw, &pub_raw);
    match result {
        Ok(secret) => {
            let arr = bytes_to_uint8array(scope, &secret);
            rv.set(arr.into());
        }
        Err(e) => throw_error(scope, &e),
    }
}

fn ecdh_compute_raw_impl(curve: &str, priv_raw: &[u8], pub_raw: &[u8]) -> Result<Vec<u8>, String> {
    use p256::elliptic_curve::ecdh::diffie_hellman;
    match curve {
        "P-256" | "prime256v1" => {
            let secret =
                p256::SecretKey::from_slice(priv_raw).map_err(|e| format!("p256 priv: {e}"))?;
            let public =
                p256::PublicKey::from_sec1_bytes(pub_raw).map_err(|e| format!("p256 pub: {e}"))?;
            let shared = diffie_hellman(secret.to_nonzero_scalar(), public.as_affine());
            Ok(shared.raw_secret_bytes().to_vec())
        }
        "P-384" | "secp384r1" => {
            let secret =
                p384::SecretKey::from_slice(priv_raw).map_err(|e| format!("p384 priv: {e}"))?;
            let public =
                p384::PublicKey::from_sec1_bytes(pub_raw).map_err(|e| format!("p384 pub: {e}"))?;
            let shared = diffie_hellman(secret.to_nonzero_scalar(), public.as_affine());
            Ok(shared.raw_secret_bytes().to_vec())
        }
        "P-521" | "secp521r1" => {
            let secret =
                p521::SecretKey::from_slice(priv_raw).map_err(|e| format!("p521 priv: {e}"))?;
            let public =
                p521::PublicKey::from_sec1_bytes(pub_raw).map_err(|e| format!("p521 pub: {e}"))?;
            let shared = diffie_hellman(secret.to_nonzero_scalar(), public.as_affine());
            Ok(shared.raw_secret_bytes().to_vec())
        }
        other => Err(format!("unsupported ecdh curve: {other}")),
    }
}

fn op_crypto_hkdf<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let digest = string_arg(scope, &args, 0);
    let Some(ikm) = bytes_arg(scope, &args, 1) else {
        throw_error(scope, "hkdf: ikm must be Uint8Array");
        return;
    };
    let Some(salt) = bytes_arg(scope, &args, 2) else {
        throw_error(scope, "hkdf: salt must be Uint8Array");
        return;
    };
    let Some(info) = bytes_arg(scope, &args, 3) else {
        throw_error(scope, "hkdf: info must be Uint8Array");
        return;
    };
    let keylen = string_arg(scope, &args, 4).parse::<usize>().unwrap_or(32);
    let result = hkdf_impl(&digest, &ikm, &salt, &info, keylen);
    match result {
        Ok(okm) => {
            let arr = bytes_to_uint8array(scope, &okm);
            rv.set(arr.into());
        }
        Err(e) => throw_error(scope, &e),
    }
}

fn hkdf_impl(
    digest: &str,
    ikm: &[u8],
    salt: &[u8],
    info: &[u8],
    keylen: usize,
) -> Result<Vec<u8>, String> {
    use hkdf::Hkdf;
    match digest {
        "sha1" => {
            let h = Hkdf::<sha1::Sha1>::new(Some(salt), ikm);
            let mut okm = vec![0u8; keylen];
            h.expand(info, &mut okm)
                .map_err(|e| format!("hkdf sha1: {e}"))?;
            Ok(okm)
        }
        "sha256" => {
            let h = Hkdf::<sha2::Sha256>::new(Some(salt), ikm);
            let mut okm = vec![0u8; keylen];
            h.expand(info, &mut okm)
                .map_err(|e| format!("hkdf sha256: {e}"))?;
            Ok(okm)
        }
        "sha384" => {
            let h = Hkdf::<sha2::Sha384>::new(Some(salt), ikm);
            let mut okm = vec![0u8; keylen];
            h.expand(info, &mut okm)
                .map_err(|e| format!("hkdf sha384: {e}"))?;
            Ok(okm)
        }
        "sha512" => {
            let h = Hkdf::<sha2::Sha512>::new(Some(salt), ikm);
            let mut okm = vec![0u8; keylen];
            h.expand(info, &mut okm)
                .map_err(|e| format!("hkdf sha512: {e}"))?;
            Ok(okm)
        }
        other => Err(format!("unsupported hkdf digest: {other}")),
    }
}

// ──────────────────────────────────────────────────────────────────────
// node:vm — real V8 contexts
// ──────────────────────────────────────────────────────────────────────

/// Property name of the hidden symbol that links a JS sandbox handed
/// to `vm.createContext` back to its `Global<Context>` slot in
/// [`super::bridge::BridgeState::vm_contexts`].
const VM_CONTEXT_ID_PROP: &str = "__nexide_vm_context_id__";

fn op_vm_create_context<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let template = v8::ObjectTemplate::new(scope);
    template.set_internal_field_count(1);

    let new_context = v8::Context::new(
        scope,
        v8::ContextOptions {
            global_template: Some(template),
            ..Default::default()
        },
    );

    let host_context = scope.get_current_context();
    let host_token = host_context.get_security_token(scope);
    new_context.set_security_token(host_token);

    let context_global = v8::Global::new(scope, new_context);
    let bridge = from_isolate(scope);
    let table = bridge.0.borrow().vm_contexts.clone();
    let id = table.insert(context_global);

    let new_global = new_context.global(scope);
    let id_external = v8::External::new(scope, id as *mut std::ffi::c_void);
    new_global.set_internal_field(0, id_external.into());

    if let Some(key) = v8::String::new(scope, VM_CONTEXT_ID_PROP) {
        let id_num = v8::Number::new(scope, f64::from(id));
        new_global.set(scope, key.into(), id_num.into());
    }

    if args.length() >= 1
        && args.get(0).is_object()
        && let Ok(seed) = TryInto::<v8::Local<v8::Object>>::try_into(args.get(0))
        && let Some(names) = seed.get_own_property_names(scope, Default::default())
    {
        let len = names.length();
        for i in 0..len {
            if let Some(key) = names.get_index(scope, i)
                && let Some(value) = seed.get(scope, key)
            {
                new_global.set(scope, key, value);
            }
        }
    }

    rv.set(new_global.into());
}

fn op_vm_run_in_context<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    if args.length() < 2 {
        throw_type_error(
            scope,
            "op_vm_run_in_context: expected (sandbox, code, filename?)",
        );
        return;
    }
    let sandbox: v8::Local<v8::Object> = match args.get(0).try_into() {
        Ok(o) => o,
        Err(_) => {
            throw_type_error(scope, "op_vm_run_in_context: sandbox must be an Object");
            return;
        }
    };
    let id = match read_vm_context_id(scope, sandbox) {
        Some(id) => id,
        None => {
            throw_error(
                scope,
                "op_vm_run_in_context: object was not produced by vm.createContext",
            );
            return;
        }
    };

    let bridge = from_isolate(scope);
    let table = bridge.0.borrow().vm_contexts.clone();
    let context_global = match table.with(id, |g| g.clone()) {
        Some(g) => g,
        None => {
            throw_error(scope, "op_vm_run_in_context: context handle not found");
            return;
        }
    };

    let code = args.get(1).to_rust_string_lossy(scope);
    let filename = if args.length() >= 3 && args.get(2).is_string() {
        args.get(2).to_rust_string_lossy(scope)
    } else {
        "[vm:runInContext]".to_owned()
    };

    let target_context = v8::Local::new(scope, &context_global);
    let mut ctx_scope = v8::ContextScope::new(scope, target_context);
    let scope_cs: &mut v8::PinScope<'_, '_> = &mut ctx_scope;

    let code_str = match v8::String::new(scope_cs, &code) {
        Some(s) => s,
        None => {
            throw_error(
                scope_cs,
                "op_vm_run_in_context: failed to allocate code string",
            );
            return;
        }
    };
    let resource = v8::String::new(scope_cs, &filename)
        .unwrap_or_else(|| v8::String::new(scope_cs, "[vm:runInContext]").expect("static name"));
    let undefined = v8::undefined(scope_cs).into();
    let origin = v8::ScriptOrigin::new(
        scope_cs,
        resource.into(),
        0,
        0,
        false,
        0,
        Some(undefined),
        false,
        false,
        false,
        None,
    );
    let mut source = v8::script_compiler::Source::new(code_str, Some(&origin));
    let script = match v8::script_compiler::compile(
        scope_cs,
        &mut source,
        v8::script_compiler::CompileOptions::NoCompileOptions,
        v8::script_compiler::NoCacheReason::NoReason,
    ) {
        Some(s) => s,
        None => {
            // Compile threw — the exception is already on the isolate
            // and propagates out of this op naturally.
            return;
        }
    };
    let result = match script.run(scope_cs) {
        Some(v) => v,
        None => return, // run threw; exception propagates
    };
    rv.set(result);
}

fn op_vm_is_context<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let is_ctx = if args.length() >= 1 {
        match TryInto::<v8::Local<v8::Object>>::try_into(args.get(0)) {
            Ok(obj) => read_vm_context_id(scope, obj).is_some(),
            Err(_) => false,
        }
    } else {
        false
    };
    rv.set(v8::Boolean::new(scope, is_ctx).into());
}

/// Reads the registry id stamped on a sandbox by
/// [`op_vm_create_context`]. Returns `None` if the object was not
/// produced by us.
fn read_vm_context_id<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    obj: v8::Local<'s, v8::Object>,
) -> Option<u32> {
    if obj.internal_field_count() < 1 {
        return None;
    }
    let raw = obj.get_internal_field(scope, 0)?;
    let external: v8::Local<v8::External> = raw.try_into().ok()?;
    let ptr = external.value();
    if ptr.is_null() {
        return None;
    }
    Some(ptr as usize as u32)
}
