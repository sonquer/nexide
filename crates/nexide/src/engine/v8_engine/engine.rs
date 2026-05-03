//! V8-backed [`crate::engine::IsolateHandle`] implementation.
//!
//! `V8Engine` owns a [`v8::OwnedIsolate`], a global [`v8::Context`]
//! and the entrypoint module. The bridge state ([`super::bridge::BridgeStateHandle`])
//! and the module map ([`super::modules::ModuleMap`]) are parked in
//! the isolate's slot store so V8 callbacks can fetch them through a
//! raw `&Isolate` reference.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Once;

use async_trait::async_trait;
use tokio::sync::oneshot;

use super::bootstrap::POLYFILL_SCRIPTS;
use super::bridge::{BridgeState, BridgeStateHandle};
use super::modules::{ModuleMap, read_module_source, resolve_relative};
use super::ops_bridge;
use crate::engine::{EngineError, HeapLimitConfig, HeapStats, IsolateHandle};
use crate::ops::{
    DispatchTable, FsHandle, ProcessConfig, RequestFailure, RequestId, RequestQueue, RequestSlot,
    ResponsePayload, WorkerId,
};

const LOG_TARGET: &str = "nexide::engine::v8";

static V8_INIT: Once = Once::new();

#[repr(C, align(16))]
struct IcuData<T: ?Sized>(T);

static ICU_DATA_RAW: &IcuData<[u8]> = &IcuData(*include_bytes!("../../../runtime/icudtl.dat"));

static ICU_DATA: &[u8] = &ICU_DATA_RAW.0;

fn ensure_v8_initialized() {
    V8_INIT.call_once(|| {
        if let Err(code) = v8::icu::set_common_data_77(ICU_DATA) {
            panic!("failed to load ICU data into V8: error code {code}");
        }
        let platform = v8::new_default_platform(0, false).make_shared();
        v8::V8::initialize_platform(platform);
        v8::V8::initialize();
        let preferred = std::env::var("LC_ALL")
            .ok()
            .or_else(|| std::env::var("LANG").ok())
            .and_then(|raw| {
                let trimmed = raw.split('.').next().unwrap_or("").trim().to_owned();
                if trimmed.is_empty() || trimmed == "C" || trimmed == "POSIX" {
                    None
                } else {
                    Some(trimmed)
                }
            })
            .unwrap_or_else(|| "en_US".to_owned());
        v8::icu::set_default_locale(&preferred);
    });
}

// ──────────────────────────────────────────────────────────────────────
// BootContext
// ──────────────────────────────────────────────────────────────────────

/// Opt-in slots passed to [`V8Engine::boot_with`].
#[derive(Default)]
pub struct BootContext {
    worker_id: Option<WorkerId>,
    heap_limit: Option<HeapLimitConfig>,
    process: Option<ProcessConfig>,
    fs: Option<FsHandle>,
    cjs: Option<std::sync::Arc<dyn crate::engine::cjs::CjsResolver>>,
    cjs_root: Option<String>,
    code_cache: Option<crate::engine::code_cache::CodeCache>,
}

impl BootContext {
    /// Empty context - every slot keeps its default.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the worker identity used by `op_nexide_log`.
    #[must_use]
    pub fn with_worker_id(mut self, id: WorkerId) -> Self {
        self.worker_id = Some(id);
        self
    }

    /// Overrides the V8 heap budget.
    #[must_use]
    pub fn with_heap_limit(mut self, limit: HeapLimitConfig) -> Self {
        self.heap_limit = Some(limit);
        self
    }

    /// Installs a `process.*` adapter.
    #[must_use]
    pub fn with_process(mut self, process: ProcessConfig) -> Self {
        self.process = Some(process);
        self
    }

    /// Installs the `node:fs` backend.
    #[must_use]
    pub fn with_fs(mut self, fs: FsHandle) -> Self {
        self.fs = Some(fs);
        self
    }

    /// Installs the CommonJS resolver used by `require`.
    #[must_use]
    pub fn with_cjs(
        mut self,
        resolver: std::sync::Arc<dyn crate::engine::cjs::CjsResolver>,
    ) -> Self {
        self.cjs = Some(resolver);
        self
    }

    /// Overrides the wire string returned by `op_cjs_root_parent`.
    #[must_use]
    pub fn with_cjs_root(mut self, root: impl Into<String>) -> Self {
        self.cjs_root = Some(root.into());
        self
    }

    /// Installs a persistent V8 bytecode cache shared across every
    /// compile site that opts in (see
    /// [`crate::engine::code_cache::CodeCache`]).
    #[must_use]
    pub fn with_code_cache(mut self, cache: crate::engine::code_cache::CodeCache) -> Self {
        self.code_cache = Some(cache);
        self
    }
}

// ──────────────────────────────────────────────────────────────────────
// V8Engine
// ──────────────────────────────────────────────────────────────────────

/// Concrete V8 engine.
///
/// `!Send` - V8 isolates are pinned to the thread that created them.
/// The pool uses `LocalSet` / dedicated OS threads to honour that.
pub struct V8Engine {
    isolate: v8::OwnedIsolate,
    context: v8::Global<v8::Context>,
    entrypoint: PathBuf,
    last_stats: HeapStats,
}

impl V8Engine {
    /// Boots the engine and runs the entrypoint module.
    pub async fn boot_with(entrypoint: &Path, ctx: BootContext) -> Result<Self, EngineError> {
        Self::boot_internal(entrypoint, ctx)
    }

    /// Boots the engine. Polyfills are baked into the build via
    /// [`super::bootstrap::POLYFILL_SCRIPTS`]; the iterator is
    /// retained for API compatibility but not consulted.
    pub async fn boot_with_polyfills<P>(
        entrypoint: &Path,
        ctx: BootContext,
        _polyfills: P,
    ) -> Result<Self, EngineError>
    where
        P: IntoIterator,
    {
        Self::boot_internal(entrypoint, ctx)
    }

    fn boot_internal(entrypoint: &Path, ctx: BootContext) -> Result<Self, EngineError> {
        ensure_v8_initialized();

        let entry_path =
            std::fs::canonicalize(entrypoint).map_err(|_| EngineError::ModuleResolution {
                path: entrypoint.to_path_buf(),
            })?;

        let worker_id = ctx.worker_id.unwrap_or_else(|| WorkerId::new(0, true));
        tracing::debug!(
            target: LOG_TARGET,
            worker = worker_id.id,
            primary = worker_id.is_primary,
            entry = %entry_path.display(),
            "v8 isolate boot starting",
        );

        let heap_limit = ctx.heap_limit.unwrap_or_default();
        let create_params = heap_limit.to_create_params();
        let mut isolate = v8::Isolate::new(create_params);

        let napi_wakeup = std::sync::Arc::new(tokio::sync::Notify::new());
        let channel = super::async_ops::CompletionChannel::new(std::sync::Arc::clone(&napi_wakeup));
        let (napi_tx, napi_rx) = tokio::sync::mpsc::unbounded_channel();
        let bridge = BridgeState {
            queue: Rc::new(RequestQueue::new()),
            dispatch_table: DispatchTable::default(),
            worker_id,
            process: ctx.process,
            env_overlay: crate::ops::EnvOverlay::default(),
            fs: ctx.fs,
            cjs: ctx.cjs,
            cjs_root: ctx
                .cjs_root
                .unwrap_or_else(|| crate::engine::cjs::ROOT_PARENT.to_owned()),
            exit_requested: None,
            pending_pop: std::collections::VecDeque::new(),
            pending_pop_batch: std::collections::VecDeque::new(),
            async_completions_tx: channel.sender(),
            async_completions_rx: channel.receiver(),
            napi_work_tx: napi_tx,
            napi_work_rx: Rc::new(RefCell::new(napi_rx)),
            napi_wakeup,
            net_streams: super::handle_table::HandleTable::default(),
            net_listeners: super::handle_table::HandleTable::default(),
            tls_streams: super::handle_table::HandleTable::default(),
            http_responses: super::handle_table::HandleTable::default(),
            child_processes: super::handle_table::HandleTable::default(),
            zlib_streams: super::handle_table::HandleTable::default(),
            vm_contexts: super::handle_table::HandleTable::default(),
        };
        isolate.set_slot(BridgeStateHandle::new(bridge));
        isolate.set_slot(ModuleMap::new());
        isolate.set_slot(
            ctx.code_cache
                .unwrap_or_else(crate::engine::code_cache::CodeCache::disabled),
        );
        isolate.set_host_import_module_dynamically_callback(host_import_module_dynamically);

        let context_global = {
            v8::scope!(let scope, &mut isolate);
            let context = v8::Context::new(scope, v8::ContextOptions::default());
            let scope_cs = &mut v8::ContextScope::new(scope, context);

            ops_bridge::install(scope_cs, context);
            run_polyfill_bootstrap(scope_cs)?;
            let use_cjs = scope_cs
                .get_slot::<BridgeStateHandle>()
                .cloned()
                .expect("bridge state must be installed")
                .0
                .borrow()
                .cjs
                .is_some();
            if use_cjs {
                let path_lit =
                    serde_json::to_string(&entry_path.to_string_lossy()).map_err(|err| {
                        EngineError::Bootstrap {
                            message: format!("entrypoint json encode: {err}"),
                        }
                    })?;
                let src = format!("globalThis.__nexideCjs.load(\"<root>\", {path_lit});");
                eval_script(scope_cs, "[nexide:entrypoint]", &src)?;
            } else {
                load_and_run_entrypoint(scope_cs, &entry_path)?;
            }

            v8::Global::new(scope_cs, context)
        };

        let stats = capture_heap_stats(&mut isolate);

        tracing::debug!(
            target: LOG_TARGET,
            worker = worker_id.id,
            entry = %entry_path.display(),
            heap_used = stats.used_heap_size,
            heap_total = stats.total_heap_size,
            "v8 isolate boot complete",
        );

        Ok(Self {
            isolate,
            context: context_global,
            entrypoint: entry_path,
            last_stats: stats,
        })
    }

    /// Returns a mutable reference to the underlying isolate.
    pub fn isolate_mut(&mut self) -> &mut v8::OwnedIsolate {
        &mut self.isolate
    }

    /// Returns the entrypoint absolute path.
    pub fn entrypoint(&self) -> &Path {
        &self.entrypoint
    }

    /// Returns the global context handle.
    pub fn context(&self) -> &v8::Global<v8::Context> {
        &self.context
    }

    /// Pushes a request into the queue and registers the caller's
    /// completion oneshot. Returns the [`RequestId`] handed to JS.
    pub fn enqueue_with(
        &self,
        slot: RequestSlot,
        completion: oneshot::Sender<Result<ResponsePayload, RequestFailure>>,
    ) -> RequestId {
        let handle = self
            .isolate
            .get_slot::<BridgeStateHandle>()
            .cloned()
            .expect("bridge state must be installed");
        let mut state = handle.0.borrow_mut();
        let id = state.dispatch_table.insert(slot, completion);
        state.queue.push(id);
        id
    }

    /// Convenience wrapper around [`Self::enqueue_with`] that builds
    /// the completion oneshot on the fly.
    #[must_use]
    pub fn enqueue(
        &self,
        slot: RequestSlot,
    ) -> oneshot::Receiver<Result<ResponsePayload, RequestFailure>> {
        let (tx, rx) = oneshot::channel();
        self.enqueue_with(slot, tx);
        rx
    }

    /// Drains the dispatch table, failing every in-flight request
    /// with `RequestFailure::PumpDied(reason)`.
    pub fn fail_inflight(&self, reason: &str) {
        let handle = self
            .isolate
            .get_slot::<BridgeStateHandle>()
            .cloned()
            .expect("bridge state must be installed");
        let reason_owned = reason.to_owned();
        handle
            .0
            .borrow_mut()
            .dispatch_table
            .fail_all(move || RequestFailure::PumpDied(reason_owned.clone()));
    }

    /// Starts the JavaScript-side request pump.
    ///
    /// `batch_cap == 0 || 1` selects the serial pump; values `>= 2`
    /// select the batched pump. The cap is forwarded verbatim - the
    /// JS side clamps it again on the op boundary.
    pub fn start_pump(&mut self, batch_cap: usize) -> Result<(), EngineError> {
        let cap = u32::try_from(batch_cap).unwrap_or(u32::MAX);
        let source = format!("globalThis.__nexide.__startPump({cap});");
        self.execute("[nexide:start-pump]", &source)
    }

    /// Evaluates a classic script in the engine's main realm.
    pub fn execute(&mut self, name: &str, source: &str) -> Result<(), EngineError> {
        tracing::trace!(
            target: LOG_TARGET,
            script = name,
            bytes = source.len(),
            "execute classic script",
        );
        let context = self.context.clone();
        v8::scope!(let scope, &mut self.isolate);
        let context = v8::Local::new(scope, context);
        let scope_cs = &mut v8::ContextScope::new(scope, context);
        eval_script(scope_cs, name, source).map_err(|e| {
            tracing::warn!(
                target: LOG_TARGET,
                script = name,
                error = %e,
                "classic script failed",
            );
            e
        })
    }

    /// Returns `true` when no requests are queued and there are no
    /// in-flight handlers awaiting completion.
    pub fn queue_is_empty(&self) -> bool {
        let handle = self
            .isolate
            .get_slot::<BridgeStateHandle>()
            .cloned()
            .expect("bridge state must be installed");
        let state = handle.0.borrow();
        state.queue.is_empty()
    }

    /// Returns the cross-thread wake-up handle that worker threads
    /// (e.g. N-API threadsafe-function callers) bump to make the engine
    /// pump come out of its idle parking.
    pub fn napi_wakeup(&self) -> std::sync::Arc<tokio::sync::Notify> {
        let handle = self
            .isolate
            .get_slot::<BridgeStateHandle>()
            .cloned()
            .expect("bridge state must be installed");
        handle.0.borrow().napi_wakeup.clone()
    }

    /// Resolves pending pop-request promises for any newly-arrived
    /// requests, settles every completed asynchronous op, runs a
    /// microtask checkpoint, and refreshes [`Self::heap_stats`].
    /// Cheap; safe to call from a hot loop.
    pub fn pump_once(&mut self) {
        let context = self.context.clone();
        let (async_rx, napi_rx) = {
            let handle = self
                .isolate
                .get_slot::<BridgeStateHandle>()
                .cloned()
                .expect("bridge state must be installed");
            let bridge = handle.0.borrow();
            (
                bridge.async_completions_rx.clone(),
                bridge.napi_work_rx.clone(),
            )
        };
        {
            v8::scope!(let scope, &mut self.isolate);
            let context = v8::Local::new(scope, context);
            let scope_cs = &mut v8::ContextScope::new(scope, context);
            super::async_ops::drain(scope_cs, &async_rx);
            drain_napi_work(scope_cs, &napi_rx);
            ops_bridge::drain_pending_pops(scope_cs);
        }
        self.isolate.perform_microtask_checkpoint();
        self.last_stats = capture_heap_stats(&mut self.isolate);
    }

    /// Hints V8 that the host is under memory pressure so the isolate
    /// should give back as much heap as it can right now.
    ///
    /// Calls
    /// `v8::Isolate::MemoryPressureNotification(MemoryPressureLevel::Critical)`
    /// which triggers a major GC and asks V8 to release reclaimed
    /// pages back to the OS - the only way for an idle Node-style
    /// isolate to drop its working-set RSS below the high-water mark
    /// reached during peak traffic. Combined with jemalloc decay (or
    /// `malloc_trim` on glibc), this lets a long-idle worker shrink
    /// from `100-150` MiB back down to the cold-boot baseline
    /// (~`30-40` MiB) without restarting the isolate.
    ///
    /// Cheap to call (one virtual call into V8 + one major GC pause
    /// of `~5-30` ms depending on heap size). The caller is
    /// responsible for rate-limiting; the typical pattern is to
    /// invoke this once after the dispatch queue has been empty for
    /// `NEXIDE_IDLE_GC_MS` (default `30_000` ms - see
    /// [`run_pump`](crate::pool::engine_pump) for the exact wiring).
    pub fn notify_low_memory(&mut self) {
        self.isolate
            .memory_pressure_notification(v8::MemoryPressureLevel::Critical);
        self.last_stats = capture_heap_stats(&mut self.isolate);
    }
}

fn drain_napi_work<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    rx: &Rc<RefCell<tokio::sync::mpsc::UnboundedReceiver<super::bridge::NapiWorkItem>>>,
) {
    loop {
        let next = rx.borrow_mut().try_recv();
        match next {
            Ok(work) => work(scope),
            Err(_) => return,
        }
    }
}

#[async_trait(?Send)]
impl IsolateHandle for V8Engine {
    async fn boot(entrypoint: &Path) -> Result<Self, EngineError> {
        Self::boot_with(entrypoint, BootContext::new()).await
    }

    async fn pump(&mut self) -> Result<(), EngineError> {
        self.pump_once();
        Ok(())
    }

    fn heap_stats(&self) -> HeapStats {
        self.last_stats
    }
}

// ──────────────────────────────────────────────────────────────────────
// helpers
// ──────────────────────────────────────────────────────────────────────

fn capture_heap_stats(isolate: &mut v8::Isolate) -> HeapStats {
    let stats = isolate.get_heap_statistics();
    HeapStats {
        used_heap_size: stats.used_heap_size(),
        total_heap_size: stats.total_heap_size(),
        heap_size_limit: stats.heap_size_limit(),
    }
}

fn run_polyfill_bootstrap<'s>(scope: &mut v8::PinScope<'s, '_>) -> Result<(), EngineError> {
    for (name, src) in POLYFILL_SCRIPTS {
        eval_script(scope, name, src)?;
    }
    Ok(())
}

fn eval_script<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    name: &str,
    source: &str,
) -> Result<(), EngineError> {
    let code = v8::String::new(scope, source).ok_or_else(|| EngineError::Bootstrap {
        message: format!("could not allocate string for {name}"),
    })?;
    let resource = v8::String::new(scope, name).unwrap();
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
    let mut source_obj = v8::script_compiler::Source::new(code, Some(&origin));
    let script = v8::script_compiler::compile(
        scope,
        &mut source_obj,
        v8::script_compiler::CompileOptions::NoCompileOptions,
        v8::script_compiler::NoCacheReason::NoReason,
    )
    .ok_or_else(|| EngineError::JsRuntime {
        message: format!("compile failed for {name}"),
    })?;
    if script.run(scope).is_none() {
        return Err(EngineError::JsRuntime {
            message: format!("run failed for {name}"),
        });
    }
    Ok(())
}

pub(super) fn load_and_run_entrypoint<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    entry: &Path,
) -> Result<(), EngineError> {
    let module = compile_module(scope, entry)?;
    let success = module
        .instantiate_module(scope, resolve_module_callback)
        .unwrap_or(false);
    if !success {
        return Err(EngineError::JsRuntime {
            message: format!("instantiate failed for {}", entry.display()),
        });
    }
    v8::tc_scope!(let tc, scope);
    let evaluated = module.evaluate(tc).is_some();
    if !evaluated {
        let message = tc
            .exception()
            .and_then(|e| e.to_string(tc))
            .map(|s| s.to_rust_string_lossy(tc))
            .unwrap_or_else(|| format!("evaluate returned none for {}", entry.display()));
        return Err(EngineError::JsRuntime { message });
    }
    if matches!(module.get_status(), v8::ModuleStatus::Errored) {
        let exception = module.get_exception();
        let message = exception
            .to_string(tc)
            .map(|s| s.to_rust_string_lossy(tc))
            .unwrap_or_else(|| format!("module error in {}", entry.display()));
        return Err(EngineError::JsRuntime { message });
    }
    Ok(())
}

pub(super) fn compile_module<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    path: &Path,
) -> Result<v8::Local<'s, v8::Module>, EngineError> {
    let source_text = read_module_source(path)?;
    let resource_name = path.to_string_lossy().into_owned();

    let code = v8::String::new(scope, &source_text).ok_or_else(|| EngineError::Bootstrap {
        message: format!("string alloc failed for {}", path.display()),
    })?;
    let resource = v8::String::new(scope, &resource_name).unwrap();
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
        true,
        None,
    );

    let cache = code_cache_from_isolate(scope);
    let cached_bytes = cache
        .as_ref()
        .filter(|c| c.is_enabled())
        .and_then(|c| c.lookup(&source_text));

    let (mut source, options) = match cached_bytes {
        Some(bytes) => {
            let cached = v8::script_compiler::CachedData::new(&bytes);
            (
                v8::script_compiler::Source::new_with_cached_data(code, Some(&origin), cached),
                v8::script_compiler::CompileOptions::ConsumeCodeCache,
            )
        }
        None => (
            v8::script_compiler::Source::new(code, Some(&origin)),
            v8::script_compiler::CompileOptions::NoCompileOptions,
        ),
    };

    let module = v8::script_compiler::compile_module2(
        scope,
        &mut source,
        options,
        v8::script_compiler::NoCacheReason::NoReason,
    )
    .ok_or_else(|| EngineError::JsRuntime {
        message: format!("compile failed for {}", path.display()),
    })?;

    let fresh_blob = module
        .get_unbound_module_script(scope)
        .create_code_cache()
        .map(|cd| cd.to_vec());

    finalise_cache_after_compile(cache.as_ref(), &source, &source_text, options, fresh_blob);

    let hash = module.get_identity_hash().get();
    let module_global = v8::Global::new(scope, module);
    if let Some(map) = get_module_map_mut(scope) {
        map.insert(path.to_path_buf(), hash, module_global);
    }
    Ok(module)
}

/// Returns the [`code_cache::CodeCache`] attached to `isolate`, or
/// `None` when no cache slot has been installed (e.g. in unit tests
/// that bypass [`V8Engine::boot_internal`]).
pub(super) fn code_cache_from_isolate<'s>(
    scope: &v8::PinScope<'s, '_>,
) -> Option<crate::engine::code_cache::CodeCache> {
    scope
        .get_slot::<crate::engine::code_cache::CodeCache>()
        .cloned()
}

fn finalise_cache_after_compile(
    cache: Option<&crate::engine::code_cache::CodeCache>,
    source_obj: &v8::script_compiler::Source,
    source_text: &str,
    options: v8::script_compiler::CompileOptions,
    fresh_blob: Option<Vec<u8>>,
) {
    let Some(cache) = cache else { return };
    if !cache.is_enabled() {
        return;
    }

    let consumed = options.contains(v8::script_compiler::CompileOptions::ConsumeCodeCache);
    let rejected = source_obj
        .get_cached_data()
        .map(v8::CachedData::rejected)
        .unwrap_or(false);

    if consumed && !rejected {
        return;
    }

    if consumed && rejected {
        cache.metrics().record_reject();
    }

    if let Some(blob) = fresh_blob.filter(|b| !b.is_empty()) {
        cache.store(source_text, blob);
    }
}

/// Returns `Some(&mut ModuleMap)` from the isolate's slot store.
pub(super) fn get_module_map_mut<'s, 'a>(
    scope: &'a mut v8::PinScope<'s, '_>,
) -> Option<&'a mut ModuleMap> {
    scope.get_slot_mut::<ModuleMap>()
}

#[allow(clippy::unnecessary_wraps)]
pub(super) fn resolve_module_callback<'s>(
    context: v8::Local<'s, v8::Context>,
    specifier: v8::Local<'s, v8::String>,
    _import_attributes: v8::Local<'s, v8::FixedArray>,
    referrer: v8::Local<'s, v8::Module>,
) -> Option<v8::Local<'s, v8::Module>> {
    v8::callback_scope!(unsafe scope, context);

    let specifier_str = specifier.to_rust_string_lossy(scope);
    let referrer_hash = referrer.get_identity_hash().get();

    // Fast path: ESM loader pre-resolved this dependency and stored
    // the absolute key path in the module map.
    let resolved_key: Option<PathBuf> = scope.get_slot::<ModuleMap>().and_then(|m| {
        m.lookup_resolution(referrer_hash, &specifier_str)
            .map(Path::to_path_buf)
    });
    if let Some(key) = resolved_key {
        let cached: Option<v8::Global<v8::Module>> = scope
            .get_slot::<ModuleMap>()
            .and_then(|m: &ModuleMap| m.get(&key).cloned());
        if let Some(g) = cached {
            return Some(v8::Local::new(scope, &g));
        }
    }

    let parent_path: Option<PathBuf> = scope
        .get_slot::<ModuleMap>()
        .and_then(|m: &ModuleMap| m.path_of_hash(referrer_hash).map(Path::to_path_buf));

    let Some(parent) = parent_path else {
        throw_error(scope, "resolve: unknown referrer module");
        return None;
    };

    let resolved = resolve_relative(&parent, &specifier_str);

    let cached: Option<v8::Global<v8::Module>> = scope
        .get_slot::<ModuleMap>()
        .and_then(|m: &ModuleMap| m.get(&resolved).cloned());
    if let Some(g) = cached {
        return Some(v8::Local::new(scope, &g));
    }

    match compile_module(scope, &resolved) {
        Ok(module) => Some(module),
        Err(err) => {
            throw_error(scope, &err.to_string());
            None
        }
    }
}

pub(super) fn throw_error<'s>(scope: &mut v8::PinScope<'s, '_>, message: &str) {
    let msg = v8::String::new(scope, message).unwrap();
    let exc = v8::Exception::error(scope, msg);
    scope.throw_exception(exc);
}

/// V8 host hook for `import(specifier)` expressions. Bridges to the
/// real ESM loader in [`super::esm`] which:
///
/// * resolves the specifier with ESM conditions,
/// * compiles + instantiates real `.mjs` graphs,
/// * wraps any CJS dependency as a synthetic V8 module (default
///   export = `module.exports`),
/// * settles the returned promise with the module namespace once the
///   evaluate promise (top-level await aware) fulfils.
///
/// CJS-only callers fall through to `__nexideCjs.dynamicImport` via
/// the `op_esm_dynamic_import` op the JS shim invokes.
fn host_import_module_dynamically<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _host_defined_options: v8::Local<'s, v8::Data>,
    resource_name: v8::Local<'s, v8::Value>,
    specifier: v8::Local<'s, v8::String>,
    _import_attributes: v8::Local<'s, v8::FixedArray>,
) -> Option<v8::Local<'s, v8::Promise>> {
    let specifier_str = specifier.to_rust_string_lossy(scope);
    let referrer_str = if resource_name.is_string() {
        Some(resource_name.to_rust_string_lossy(scope))
    } else {
        None
    };
    tracing::trace!(
        target: LOG_TARGET,
        specifier = %specifier_str,
        referrer = ?referrer_str,
        "host_import_module_dynamically",
    );
    let promise = super::esm::do_esm_dynamic_import(scope, &specifier_str, referrer_str.as_deref());
    Some(promise)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn write_temp(contents: &str) -> NamedTempFile {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(contents.as_bytes()).unwrap();
        file
    }

    #[tokio::test(flavor = "current_thread")]
    async fn boot_runs_a_trivial_module() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let file = write_temp("globalThis.__nexide_test_marker = 1;\n");
                let engine = V8Engine::boot(file.path()).await;
                assert!(engine.is_ok(), "boot failed: {:?}", engine.err());
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn missing_entrypoint_is_module_resolution_error() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let path = std::env::temp_dir().join("nexide-nonexistent-entry-xyzzy.mjs");
                let result = V8Engine::boot(&path).await;
                assert!(matches!(result, Err(EngineError::ModuleResolution { .. })));
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pump_is_idempotent() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let file = write_temp("globalThis.x = 0;\n");
                let mut engine = V8Engine::boot(file.path()).await.unwrap();
                engine.pump().await.unwrap();
                engine.pump().await.unwrap();
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn text_decoder_streams_split_utf8() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let src = r#"
                  const json = JSON.stringify({"hello":"świecie","poly":"łódź żółć","amount":"1 234 567,89"});
                  const bytes = new TextEncoder().encode(json);
                  for (const chunkSize of [1, 2, 3, 5, 7, 11, 13, 17]) {
                    const td = new TextDecoder("utf-8");
                    let out = "";
                    for (let i = 0; i < bytes.length; i += chunkSize) {
                      out += td.decode(bytes.subarray(i, Math.min(i + chunkSize, bytes.length)), { stream: true });
                    }
                    out += td.decode();
                    if (out !== json) {
                      throw new Error("mismatch at chunkSize=" + chunkSize + ":\nexpected: " + json + "\ngot:      " + out);
                    }
                    JSON.parse(out);
                  }
                "#;
                let file = write_temp(src);
                let result = V8Engine::boot(file.path()).await;
                assert!(result.is_ok(), "TextDecoder streaming failed: {:?}", result.err());
            })
            .await;
    }
}
