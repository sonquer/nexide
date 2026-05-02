//! Real ESM dynamic-import support.
//!
//! The flow mirrors Node.js / Deno semantics:
//!
//! 1. The V8 host hook (in [`super::engine`]) and the JS-level
//!    `__nexideCjs.dynamicImport` shim both forward to
//!    [`do_esm_dynamic_import`] / [`op_esm_dynamic_import`].
//! 2. Specifiers are resolved through the CJS resolver using ESM
//!    conditions (`["node", "import", "default"]`).
//! 3. ESM files (`.mjs` or `.js` with `package.json#type == "module"`)
//!    are compiled via `v8::script_compiler::compile_module`. Their
//!    static `import` graph is walked depth-first and pre-compiled
//!    with cycle protection (the [`super::modules::ModuleMap`] holds a
//!    handle to a partially-initialised module; the resolve callback
//!    accepts uninstantiated modules during linking).
//! 4. CJS dependencies referenced from ESM are loaded through the
//!    existing CJS loader (`__nexideCjs.load`) and wrapped in a
//!    synthetic V8 module whose `default` export is the CJS
//!    `module.exports` and whose named exports mirror its enumerable
//!    own properties.
//! 5. After instantiate + evaluate, the outer dynamic-import promise
//!    is settled with the module namespace once the evaluate promise
//!    (which may be pending under top-level await) fulfils.
//!
//! Known gaps:
//! * `import.meta.url` is not yet wired through `compile_module`.
//! * Synthetic modules import-from-ESM-back-to-CJS cycles are
//!   resolved through whatever the CJS cache already contains.
//! * Worker-thread isolates re-resolve every dependency; no shared
//!   compilation cache across isolates.

use std::path::{Path, PathBuf};

use super::bridge::from_isolate;
use super::engine::{compile_module, get_module_map_mut, resolve_module_callback};
use super::modules::{ModuleMap, resolve_relative};
use crate::engine::EngineError;
use crate::engine::cjs::{self, Resolved, is_esm_path};

const LOG_TARGET: &str = "nexide::engine::esm";

/// Public entry point used from the V8 host hook and from
/// `op_esm_dynamic_import`. Always returns a promise; failures are
/// rejected, never thrown.
pub(super) fn do_esm_dynamic_import<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    specifier: &str,
    referrer: Option<&str>,
) -> v8::Local<'s, v8::Promise> {
    let outer = v8::PromiseResolver::new(scope).expect("PromiseResolver::new");
    let outer_promise = outer.get_promise(scope);
    tracing::trace!(
        target: LOG_TARGET,
        specifier,
        referrer = referrer.unwrap_or("<root>"),
        "dynamic import requested",
    );
    match try_dynamic_import(scope, specifier, referrer) {
        Ok(value) => {
            tracing::debug!(
                target: LOG_TARGET,
                specifier,
                referrer = referrer.unwrap_or("<root>"),
                "dynamic import resolved",
            );
            outer.resolve(scope, value);
        }
        Err(err) => {
            tracing::warn!(
                target: LOG_TARGET,
                specifier,
                referrer = referrer.unwrap_or("<root>"),
                error = %err,
                "dynamic import failed",
            );
            let msg = format!("dynamic import: {err} (specifier '{specifier}')");
            let s = v8::String::new(scope, &msg).unwrap();
            let exc = v8::Exception::error(scope, s);
            outer.reject(scope, exc);
        }
    }
    outer_promise
}

fn try_dynamic_import<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    specifier: &str,
    referrer: Option<&str>,
) -> Result<v8::Local<'s, v8::Value>, String> {
    let parent = referrer
        .map(str::to_owned)
        .unwrap_or_else(|| cjs_root_parent(scope));

    let resolved = resolve_dependency(scope, &parent, specifier)?;
    match resolved {
        DepResolved::Esm(abs) => load_and_evaluate_esm(scope, &abs),
        DepResolved::Cjs {
            key: _,
            parent_arg,
            request_arg,
        } => {
            let exports = call_cjs_load(scope, &parent_arg, &request_arg)?;
            Ok(build_namespace_object(scope, exports))
        }
    }
}

fn cjs_root_parent<'s>(scope: &mut v8::PinScope<'s, '_>) -> String {
    let handle = from_isolate(scope);
    handle.0.borrow().cjs_root.clone()
}

fn load_and_evaluate_esm<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    abs_path: &Path,
) -> Result<v8::Local<'s, v8::Value>, String> {
    let module = load_esm_graph(scope, abs_path).map_err(|e| e.to_string())?;
    let namespace_after_eval = |scope: &mut v8::PinScope<'s, '_>,
                                module: v8::Local<'s, v8::Module>| {
        if matches!(module.get_status(), v8::ModuleStatus::Errored) {
            let exc = module.get_exception();
            return Err(value_to_string(scope, exc));
        }
        Ok(module.get_module_namespace())
    };

    if matches!(
        module.get_status(),
        v8::ModuleStatus::Evaluated | v8::ModuleStatus::Errored
    ) {
        return namespace_after_eval(scope, module);
    }

    if matches!(module.get_status(), v8::ModuleStatus::Uninstantiated) {
        v8::tc_scope!(let tc, scope);
        let ok = module
            .instantiate_module(tc, resolve_module_callback)
            .unwrap_or(false);
        if !ok {
            let exc = tc
                .exception()
                .map(|e| value_to_string(tc, e))
                .unwrap_or_else(|| format!("instantiate failed for {}", abs_path.display()));
            return Err(exc);
        }
    }

    let eval_value = {
        v8::tc_scope!(let tc, scope);
        match module.evaluate(tc) {
            Some(v) => v,
            None => {
                let exc = tc
                    .exception()
                    .map(|e| value_to_string(tc, e))
                    .unwrap_or_else(|| {
                        format!("evaluate returned none for {}", abs_path.display())
                    });
                return Err(exc);
            }
        }
    };

    if matches!(module.get_status(), v8::ModuleStatus::Errored) {
        let exc = module.get_exception();
        return Err(value_to_string(scope, exc));
    }

    let namespace = module.get_module_namespace();
    chain_namespace_after(scope, eval_value, namespace).map_err(|e| e.to_string())
}

/// Calls `globalThis.__nexideEsm.chain(evalPromise, namespace)` to
/// produce a Promise that fulfils with `namespace` once `evalPromise`
/// settles. When `eval_value` is not a Promise (e.g. classic eager
/// evaluation), the helper falls back to `Promise.resolve(namespace)`.
fn chain_namespace_after<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    eval_value: v8::Local<'s, v8::Value>,
    namespace: v8::Local<'s, v8::Value>,
) -> Result<v8::Local<'s, v8::Value>, String> {
    let context = scope.get_current_context();
    let global = context.global(scope);
    let key = v8::String::new(scope, "__nexideEsm").unwrap();
    let helper_obj = global
        .get(scope, key.into())
        .ok_or_else(|| "missing __nexideEsm".to_owned())?;
    let helper_obj: v8::Local<v8::Object> = helper_obj
        .try_into()
        .map_err(|_| "__nexideEsm is not an object".to_owned())?;
    let chain_key = v8::String::new(scope, "chain").unwrap();
    let chain_val = helper_obj
        .get(scope, chain_key.into())
        .ok_or_else(|| "missing __nexideEsm.chain".to_owned())?;
    let chain_fn: v8::Local<v8::Function> = chain_val
        .try_into()
        .map_err(|_| "__nexideEsm.chain is not a function".to_owned())?;
    v8::tc_scope!(let tc, scope);
    let recv: v8::Local<v8::Value> = helper_obj.into();
    let args = [eval_value, namespace];
    chain_fn.call(tc, recv, &args).ok_or_else(|| {
        tc.exception()
            .map(|e| value_to_string(tc, e))
            .unwrap_or_else(|| "chain helper threw".to_owned())
    })
}

/// Recursively compiles `abs_path` and all of its static imports.
/// Caches everything in [`ModuleMap`] keyed by absolute path; cycles
/// are handled by the cache check at the top.
pub(super) fn load_esm_graph<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    abs_path: &Path,
) -> Result<v8::Local<'s, v8::Module>, EngineError> {
    if let Some(cached) = get_module_map_mut(scope).and_then(|m| m.get(abs_path).cloned()) {
        tracing::trace!(
            target: LOG_TARGET,
            path = %abs_path.display(),
            "esm graph cache hit",
        );
        return Ok(v8::Local::new(scope, &cached));
    }
    tracing::debug!(
        target: LOG_TARGET,
        path = %abs_path.display(),
        "compiling esm module",
    );
    let module = compile_module(scope, abs_path).map_err(|e| {
        tracing::warn!(
            target: LOG_TARGET,
            path = %abs_path.display(),
            error = %e,
            "esm module compile failed",
        );
        e
    })?;
    process_module_requests(scope, module, abs_path)?;
    Ok(module)
}

fn process_module_requests<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    module: v8::Local<'s, v8::Module>,
    parent_path: &Path,
) -> Result<(), EngineError> {
    let parent_hash = module.get_identity_hash().get();
    let requests = module.get_module_requests();
    let len = requests.length();

    let mut specifiers: Vec<String> = Vec::with_capacity(len);
    for i in 0..len {
        let item = requests
            .get(scope, i)
            .ok_or_else(|| EngineError::JsRuntime {
                message: format!("missing module request #{i}"),
            })?;
        // V8 promises that requests entries are always ModuleRequest.
        let req: v8::Local<v8::ModuleRequest> =
            item.try_into().map_err(|_| EngineError::JsRuntime {
                message: format!("module request #{i} is not a ModuleRequest"),
            })?;
        let spec = req.get_specifier().to_rust_string_lossy(scope);
        specifiers.push(spec);
    }

    let parent_str = parent_path.to_string_lossy().into_owned();
    for spec in specifiers {
        let resolved =
            resolve_dependency(scope, &parent_str, &spec).map_err(|e| EngineError::JsRuntime {
                message: format!("ESM resolve '{spec}' from '{}': {e}", parent_path.display()),
            })?;
        match resolved {
            DepResolved::Esm(abs) => {
                if let Some(map) = get_module_map_mut(scope) {
                    map.set_resolution(parent_hash, &spec, abs.clone());
                }
                load_esm_graph(scope, &abs)?;
            }
            DepResolved::Cjs {
                key,
                parent_arg,
                request_arg,
            } => {
                ensure_synthetic_for_cjs(scope, &key, &parent_arg, &request_arg)?;
                if let Some(map) = get_module_map_mut(scope) {
                    map.set_resolution(parent_hash, &spec, key);
                }
            }
        }
    }
    Ok(())
}

#[derive(Debug)]
enum DepResolved {
    Esm(PathBuf),
    Cjs {
        /// Key under which the synthetic module is cached.
        key: PathBuf,
        /// `parent` argument to forward to `__nexideCjs.load`.
        parent_arg: String,
        /// `request` argument to forward to `__nexideCjs.load`.
        request_arg: String,
    },
}

fn resolve_dependency<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    parent: &str,
    request: &str,
) -> Result<DepResolved, String> {
    if let Some(name) = request.strip_prefix("node:") {
        return Ok(DepResolved::Cjs {
            key: PathBuf::from(format!("node:{name}")),
            parent_arg: parent.to_owned(),
            request_arg: request.to_owned(),
        });
    }

    let is_relative = request.starts_with("./")
        || request.starts_with("../")
        || request.starts_with('/')
        || request == "."
        || request == "..";

    if is_relative {
        let parent_path = Path::new(parent);
        let candidate = resolve_relative(parent_path, request);
        let resolver = cjs_resolver(scope)?;
        match resolver.resolve(parent, request) {
            Ok(Resolved::File(p)) | Ok(Resolved::Json(p)) | Ok(Resolved::Native(p)) => {
                if is_esm_path(&p) {
                    Ok(DepResolved::Esm(p))
                } else {
                    Ok(DepResolved::Cjs {
                        key: p,
                        parent_arg: parent.to_owned(),
                        request_arg: request.to_owned(),
                    })
                }
            }
            Ok(Resolved::Builtin(name)) => Ok(DepResolved::Cjs {
                key: PathBuf::from(format!("node:{name}")),
                parent_arg: parent.to_owned(),
                request_arg: request.to_owned(),
            }),
            Err(_) => {
                // Fall back to plain relative path; ESM if extension says so.
                if is_esm_path(&candidate) {
                    Ok(DepResolved::Esm(candidate))
                } else {
                    Err(format!(
                        "could not resolve relative '{request}' from '{parent}'"
                    ))
                }
            }
        }
    } else {
        let resolver = cjs_resolver(scope)?;
        let resolved = resolver
            .resolve_esm(parent, request)
            .map_err(|e| e.to_string())?;
        Ok(match resolved {
            Resolved::File(p) | Resolved::Json(p) | Resolved::Native(p) => {
                if is_esm_path(&p) {
                    DepResolved::Esm(p)
                } else {
                    DepResolved::Cjs {
                        key: p,
                        parent_arg: parent.to_owned(),
                        request_arg: request.to_owned(),
                    }
                }
            }
            Resolved::Builtin(name) => DepResolved::Cjs {
                key: PathBuf::from(format!("node:{name}")),
                parent_arg: parent.to_owned(),
                request_arg: request.to_owned(),
            },
        })
    }
}

fn cjs_resolver<'s>(
    scope: &mut v8::PinScope<'s, '_>,
) -> Result<std::sync::Arc<dyn cjs::CjsResolver>, String> {
    let handle = from_isolate(scope);
    handle
        .0
        .borrow()
        .cjs
        .clone()
        .ok_or_else(|| "cjs resolver not configured".to_owned())
}

/// Pre-loads the CJS exports for `request` and wraps them as a V8
/// SyntheticModule. Cached in [`ModuleMap`] under `key`.
fn ensure_synthetic_for_cjs<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    key: &Path,
    parent_arg: &str,
    request_arg: &str,
) -> Result<v8::Local<'s, v8::Module>, EngineError> {
    if let Some(cached) = get_module_map_mut(scope).and_then(|m| m.get(key).cloned()) {
        return Ok(v8::Local::new(scope, &cached));
    }
    let exports =
        call_cjs_load(scope, parent_arg, request_arg).map_err(|e| EngineError::JsRuntime {
            message: format!("CJS load failed for '{request_arg}': {e}"),
        })?;

    let mut export_names: Vec<v8::Local<v8::String>> = Vec::new();
    let default_name = v8::String::new(scope, "default").unwrap();
    export_names.push(default_name);

    let mut export_name_strings: Vec<String> = vec!["default".to_owned()];

    if let Ok(obj) = TryInto::<v8::Local<v8::Object>>::try_into(exports)
        && let Some(names) = obj.get_own_property_names(scope, v8::GetPropertyNamesArgs::default())
    {
        let len = names.length();
        for i in 0..len {
            if let Some(k) = names.get_index(scope, i)
                && let Some(s) = k.to_string(scope)
            {
                let rust = s.to_rust_string_lossy(scope);
                if !is_valid_export_identifier(&rust) || rust == "default" {
                    continue;
                }
                if export_name_strings.iter().any(|e| e == &rust) {
                    continue;
                }
                export_name_strings.push(rust);
            }
        }
    }

    let mut name_locals: Vec<v8::Local<v8::String>> = Vec::with_capacity(export_name_strings.len());
    for name in &export_name_strings {
        name_locals.push(v8::String::new(scope, name).unwrap());
    }

    let module_name = v8::String::new(scope, &key.to_string_lossy()).unwrap();
    let module =
        v8::Module::create_synthetic_module(scope, module_name, &name_locals, synthetic_eval_steps);

    let module_hash = module.get_identity_hash().get();
    let exports_global = v8::Global::new(scope, exports);
    if let Some(map) = get_module_map_mut(scope) {
        map.stash_synthetic_exports(module_hash, exports_global);
    }

    let module_global = v8::Global::new(scope, module);
    if let Some(map) = get_module_map_mut(scope) {
        map.insert(key.to_path_buf(), module_hash, module_global);
    }
    Ok(module)
}

fn is_valid_export_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_' || first == '$') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

#[allow(clippy::unnecessary_wraps)]
fn synthetic_eval_steps<'s>(
    context: v8::Local<'s, v8::Context>,
    module: v8::Local<'s, v8::Module>,
) -> Option<v8::Local<'s, v8::Value>> {
    v8::callback_scope!(unsafe scope, context);
    let id = module.get_identity_hash().get();
    let exports_global = scope
        .get_slot_mut::<ModuleMap>()
        .and_then(|m| m.take_synthetic_exports(id))?;
    let exports = v8::Local::new(scope, &exports_global);

    let default_name = v8::String::new(scope, "default")?;
    module.set_synthetic_module_export(scope, default_name, exports)?;

    if let Ok(obj) = TryInto::<v8::Local<v8::Object>>::try_into(exports)
        && let Some(names) = obj.get_own_property_names(scope, v8::GetPropertyNamesArgs::default())
    {
        let len = names.length();
        for i in 0..len {
            let Some(key_val) = names.get_index(scope, i) else {
                continue;
            };
            let Some(key_str) = key_val.to_string(scope) else {
                continue;
            };
            let rust = key_str.to_rust_string_lossy(scope);
            if !is_valid_export_identifier(&rust) || rust == "default" {
                continue;
            }
            let Some(value) = obj.get(scope, key_str.into()) else {
                continue;
            };
            // Ignore failures: the export name was filtered out at
            // creation time (e.g. duplicate, invalid identifier).
            let _ = module.set_synthetic_module_export(scope, key_str, value);
        }
    }
    let undef = v8::undefined(scope);
    Some(undef.into())
}

fn call_cjs_load<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    parent: &str,
    request: &str,
) -> Result<v8::Local<'s, v8::Value>, String> {
    let context = scope.get_current_context();
    let global = context.global(scope);
    let key = v8::String::new(scope, "__nexideCjs").unwrap();
    let cjs_val = global
        .get(scope, key.into())
        .ok_or_else(|| "missing __nexideCjs".to_owned())?;
    let cjs_obj: v8::Local<v8::Object> = cjs_val
        .try_into()
        .map_err(|_| "__nexideCjs is not an object".to_owned())?;
    let load_key = v8::String::new(scope, "load").unwrap();
    let load_val = cjs_obj
        .get(scope, load_key.into())
        .ok_or_else(|| "missing __nexideCjs.load".to_owned())?;
    let load_fn: v8::Local<v8::Function> = load_val
        .try_into()
        .map_err(|_| "__nexideCjs.load is not a function".to_owned())?;
    let parent_str = v8::String::new(scope, parent).unwrap();
    let request_str = v8::String::new(scope, request).unwrap();
    v8::tc_scope!(let tc, scope);
    let recv: v8::Local<v8::Value> = cjs_obj.into();
    let args = [parent_str.into(), request_str.into()];
    load_fn.call(tc, recv, &args).ok_or_else(|| {
        tc.exception()
            .map(|e| value_to_string(tc, e))
            .unwrap_or_else(|| "CJS load threw".to_owned())
    })
}

/// Builds a CJS-style namespace object: own enumerable properties of
/// `exports` plus `default = exports`. Mirrors the legacy
/// `__nexideCjs.dynamicImport` behaviour for callers expecting an
/// object instead of a real ES module namespace.
fn build_namespace_object<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    exports: v8::Local<'s, v8::Value>,
) -> v8::Local<'s, v8::Value> {
    let null_proto = v8::null(scope).into();
    let ns = v8::Object::with_prototype_and_properties(scope, null_proto, &[], &[]);
    if let Ok(obj) = TryInto::<v8::Local<v8::Object>>::try_into(exports)
        && let Some(names) = obj.get_own_property_names(scope, v8::GetPropertyNamesArgs::default())
    {
        let len = names.length();
        for i in 0..len {
            if let Some(k) = names.get_index(scope, i)
                && let Some(s) = k.to_string(scope)
                && let Some(v) = obj.get(scope, s.into())
            {
                ns.set(scope, s.into(), v);
            }
        }
    }
    let default_key = v8::String::new(scope, "default").unwrap();
    ns.set(scope, default_key.into(), exports);
    ns.into()
}

fn value_to_string<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    value: v8::Local<'s, v8::Value>,
) -> String {
    value
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_else(|| "<unprintable>".to_owned())
}

#[allow(unused)]
pub(super) fn placeholder() {}

// ──────────────────────────────────────────────────────────────────────
// Op
// ──────────────────────────────────────────────────────────────────────

pub(super) fn op_esm_dynamic_import<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let specifier = args.get(0).to_rust_string_lossy(scope);
    let referrer_arg = args.get(1);
    let referrer = if referrer_arg.is_string() {
        Some(referrer_arg.to_rust_string_lossy(scope))
    } else {
        None
    };
    let promise = do_esm_dynamic_import(scope, &specifier, referrer.as_deref());
    rv.set(promise.into());
}
