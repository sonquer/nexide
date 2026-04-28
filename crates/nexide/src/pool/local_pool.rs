//! Single-worker [`EngineDispatcher`] for `current_thread + LocalSet`
//! deployments (single-thread mode).
//!
//! ## Invariants
//!
//! * Always exactly **one** [`LocalIsolateWorker`] — the whole point
//!   of single-thread mode is to keep the entire request lifecycle on
//!   one OS thread, so a multi-slot picker would be a category error.
//! * Recycle runs in a **dedicated** [`tokio::task::spawn_local`]
//!   task booted at [`LocalIsolatePool::boot`] time and parked on a
//!   [`tokio::sync::Notify`]. Dispatch only signals the recycler; the
//!   actual `LocalIsolateWorker::spawn_local` rebuild always executes
//!   on the `LocalSet`, never on the per-connection task that
//!   `axum::serve` schedules through plain `tokio::spawn` (which is
//!   *not* `LocalSet`-aware and would panic).
//! * Public surface honours [`EngineDispatcher`] so the upstream
//!   handler/router code is mode-agnostic — the only seam between
//!   single-thread and multi-thread mode is the runtime-construction
//!   site in `crate::run`.
//!
//! See `docs/PERF_NOTES.md` (Iteracja 3) for the queueing-theory
//! rationale and the user-facing change log.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use super::local_isolate_worker::LocalIsolateWorker;
use super::recycle::{
    RecyclePolicy, build_default_recycle_policy_with, reap_heap_bytes_from_env,
    reap_heap_ratio_from_env, reap_request_count_from_env, reap_rss_bytes_from_env,
};
use super::worker::{Job, Worker, WorkerError};
use crate::dispatch::{DispatchError, EngineDispatcher, ProtoRequest};
use crate::ops::ResponsePayload;

/// Single-worker pool that runs the V8 isolate on the caller's
/// `LocalSet`.
pub struct LocalIsolatePool {
    worker: Arc<LocalIsolateWorker>,
    policy: Arc<dyn RecyclePolicy>,
    dispatch_count: AtomicUsize,
    recycle_count: Arc<AtomicUsize>,
    recycle_notify: Arc<Notify>,
    recycler: Mutex<Option<JoinHandle<()>>>,
}

impl LocalIsolatePool {
    /// Boots the pool's single worker on the current `LocalSet` using
    /// the recycle policy resolved from the environment (same env
    /// contract as [`super::IsolatePool::with_isolate_workers`]).
    ///
    /// Also spawns a long-lived recycler task on the same `LocalSet`
    /// that performs worker rebuilds when signalled — this is the
    /// only place where [`LocalIsolateWorker::spawn_local`] runs
    /// after boot, guaranteeing it always executes inside the
    /// `LocalSet` regardless of which task triggered the recycle.
    ///
    /// # Errors
    ///
    /// [`WorkerError::Engine`] if the engine fails to boot;
    /// [`WorkerError::Shutdown`] when the boot future is dropped early.
    ///
    /// # Panics
    ///
    /// Panics when called outside an active
    /// [`tokio::task::LocalSet`] (propagated from
    /// [`LocalIsolateWorker::spawn_local`] and
    /// [`tokio::task::spawn_local`]).
    pub async fn boot(
        entrypoint: PathBuf,
        worker_id: usize,
        workers: usize,
    ) -> Result<Self, WorkerError> {
        let policy = resolve_policy_from_env();
        let worker =
            Arc::new(LocalIsolateWorker::spawn_local(entrypoint, worker_id, workers).await?);
        let recycle_notify = Arc::new(Notify::new());
        let recycle_count = Arc::new(AtomicUsize::new(0));
        let handle = tokio::task::spawn_local(run_recycler(
            Arc::clone(&worker),
            Arc::clone(&policy),
            Arc::clone(&recycle_notify),
            Arc::clone(&recycle_count),
        ));
        Ok(Self {
            worker,
            policy,
            dispatch_count: AtomicUsize::new(0),
            recycle_count,
            recycle_notify,
            recycler: Mutex::new(Some(handle)),
        })
    }

    /// Returns the configured pool size. Always `1` for the local
    /// pool; exposed to mirror [`super::IsolatePool::size`] so
    /// observability code is mode-agnostic.
    #[must_use]
    pub const fn size(&self) -> usize {
        1
    }

    /// Returns the number of times the worker has been recycled
    /// since boot (Query — pure).
    #[must_use]
    pub fn recycle_count(&self) -> usize {
        self.recycle_count.load(Ordering::Relaxed)
    }

    async fn dispatch_inner(
        &self,
        request: ProtoRequest,
    ) -> Result<ResponsePayload, DispatchError> {
        let outcome = self.worker.dispatch(Job { request }).await;
        match outcome {
            Ok(payload) => {
                self.dispatch_count.fetch_add(1, Ordering::Relaxed);
                let snapshot = self.worker.health();
                if self.policy.should_recycle(&snapshot) {
                    self.recycle_notify.notify_one();
                }
                Ok(payload)
            }
            Err(err) => Err(map_worker_error(err)),
        }
    }
}

impl Drop for LocalIsolatePool {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.recycler.lock()
            && let Some(handle) = guard.take()
        {
            handle.abort();
        }
    }
}

async fn run_recycler(
    worker: Arc<LocalIsolateWorker>,
    policy: Arc<dyn RecyclePolicy>,
    notify: Arc<Notify>,
    recycle_count: Arc<AtomicUsize>,
) {
    loop {
        notify.notified().await;
        let snapshot = worker.health();
        if !policy.should_recycle(&snapshot) {
            continue;
        }
        match worker.rebuild().await {
            Ok(()) => {
                recycle_count.fetch_add(1, Ordering::Relaxed);
                tracing::info!(
                    heap_used = snapshot.heap.used_heap_size,
                    heap_limit = snapshot.heap.heap_size_limit,
                    requests = snapshot.requests_handled,
                    "local worker recycled (single-thread mode)"
                );
            }
            Err(err) => {
                tracing::error!(
                    error = %err,
                    "local worker recycle failed — keeping the existing isolate"
                );
            }
        }
    }
}

#[async_trait]
impl EngineDispatcher for LocalIsolatePool {
    async fn dispatch(&self, request: ProtoRequest) -> Result<ResponsePayload, DispatchError> {
        self.dispatch_inner(request).await
    }

    fn dispatch_count(&self) -> usize {
        self.dispatch_count.load(Ordering::Relaxed)
    }
}

fn map_worker_error(err: WorkerError) -> DispatchError {
    match err {
        WorkerError::Shutdown => DispatchError::WorkerGone,
        WorkerError::BadRequest(msg) | WorkerError::Engine(msg) => DispatchError::BodyRead(msg),
    }
}

fn resolve_policy_from_env() -> Arc<dyn RecyclePolicy> {
    let heap_raw = std::env::var("NEXIDE_REAP_HEAP_RATIO").ok();
    let req_raw = std::env::var("NEXIDE_REAP_AFTER_REQUESTS").ok();
    let heap_mb_raw = std::env::var("NEXIDE_REAP_HEAP_MB").ok();
    let rss_mb_raw = std::env::var("NEXIDE_REAP_RSS_MB").ok();
    let heap_ratio = reap_heap_ratio_from_env(heap_raw.as_deref());
    let request_count = reap_request_count_from_env(req_raw.as_deref());
    let heap_bytes = reap_heap_bytes_from_env(heap_mb_raw.as_deref());
    let rss_bytes = reap_rss_bytes_from_env(rss_mb_raw.as_deref());
    if heap_raw.is_some() && heap_ratio.is_none() {
        tracing::warn!(value = ?heap_raw, "NEXIDE_REAP_HEAP_RATIO unparseable — using default");
    }
    if req_raw.is_some() && request_count.is_none() {
        tracing::warn!(value = ?req_raw, "NEXIDE_REAP_AFTER_REQUESTS unparseable — using default");
    }
    if heap_mb_raw.is_some() && heap_bytes.is_none() {
        tracing::warn!(value = ?heap_mb_raw, "NEXIDE_REAP_HEAP_MB unparseable — ignoring");
    }
    if rss_mb_raw.is_some() && rss_bytes.is_none() {
        tracing::warn!(value = ?rss_mb_raw, "NEXIDE_REAP_RSS_MB unparseable — ignoring");
    }
    let sampler = rss_bytes
        .filter(|n| *n > 0)
        .map(|_| super::mem_sampler::ProcessSampler::live());
    build_default_recycle_policy_with(heap_ratio, request_count, heap_bytes, rss_bytes, sampler)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn never_recycle() -> std::sync::Arc<dyn RecyclePolicy> {
        struct Never;
        impl RecyclePolicy for Never {
            fn should_recycle(&self, _: &super::super::worker::WorkerHealth) -> bool {
                false
            }
        }
        std::sync::Arc::new(Never)
    }

    #[test]
    fn map_worker_error_shutdown_to_worker_gone() {
        match map_worker_error(WorkerError::Shutdown) {
            DispatchError::WorkerGone => {}
            other => panic!("expected WorkerGone, got {other:?}"),
        }
    }

    #[test]
    fn map_worker_error_engine_to_body_read() {
        match map_worker_error(WorkerError::Engine("boom".into())) {
            DispatchError::BodyRead(msg) => assert_eq!(msg, "boom"),
            other => panic!("expected BodyRead, got {other:?}"),
        }
    }

    #[test]
    fn resolve_policy_returns_default_when_env_clear() {
        for var in [
            "NEXIDE_REAP_HEAP_RATIO",
            "NEXIDE_REAP_AFTER_REQUESTS",
            "NEXIDE_REAP_HEAP_MB",
            "NEXIDE_REAP_RSS_MB",
        ] {
            // SAFETY: tests in this binary share env state; this
            // closure is single-threaded inside the test thread and
            // does not race with any other resolver call.
            unsafe {
                std::env::remove_var(var);
            }
        }
        let _ = resolve_policy_from_env();
    }

    #[test]
    fn never_recycle_policy_never_fires() {
        let policy = never_recycle();
        assert!(!policy.should_recycle(&super::super::worker::WorkerHealth::fresh()));
    }
}
