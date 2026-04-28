//! `IsolatePool` — multi-worker pool with lock-free dispatch and
//! background recycling.
//!
//! ## Architecture
//!
//! Each slot stores its worker behind a `std::sync::Mutex<Arc<dyn
//! Worker>>` that is locked only long enough to clone the `Arc` (a
//! single atomic increment) — effectively wait-free for dispatchers.
//! The mutex is never held across `await`, so there is no head-of-line
//! blocking: many tasks can race through the lock, each leaving with
//! its own snapshot of the current worker. Inflight load is tracked
//! per slot via an [`AtomicUsize`] counter, decremented automatically
//! by an RAII guard whether the dispatch future completes or is
//! cancelled.
//!
//! ## Picker
//!
//! [`PoolInner::pick_index`] implements a "power of two choices"
//! variant: it samples two distinct indices via two independent atomic
//! counters with coprime strides and picks the one with the lower
//! inflight count. Under fully sequential load (test scenarios) the
//! ties resolve to the first counter, giving a strict round-robin —
//! tests can therefore assert deterministic ordering. Under concurrent
//! load the picker biases towards the less loaded slot, eliminating
//! the head-of-line blocking that a plain round-robin suffers when
//! one slot is slower than its peers.
//!
//! ## Recycling
//!
//! Recycling is moved off the dispatch path entirely. A background
//! tokio task wakes periodically (`RECYCLE_TICK`) and rebuilds slots
//! whose health snapshot violates the configured [`RecyclePolicy`].
//! Each slot has a singleflight `recycling: AtomicBool` guard so the
//! background task never starts more than one rebuild per slot at a
//! time, and the freshly built worker is published with a single
//! mutex swap — in-flight dispatches against the previous worker
//! keep their own [`Arc`] alive and complete on it without
//! interruption.
//!
//! Tests can drive recycling deterministically via
//! [`IsolatePool::tick_recycler`], which runs exactly one pass and
//! awaits its completion.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::Weak;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use tokio::task::JoinHandle;

use super::isolate_worker::IsolateWorker;
use super::recycle::{
    RecyclePolicy, reap_heap_ratio_from_env,
    reap_request_count_from_env,
};
use super::worker::{Job, Worker, WorkerError, WorkerHealth};
use crate::dispatch::{DispatchError, EngineDispatcher, ProtoRequest};
use crate::ops::ResponsePayload;

/// Background recycler tick interval. Short enough that policy
/// violations are acted on quickly under sustained traffic, long
/// enough that the recycler itself adds negligible CPU overhead.
const RECYCLE_TICK: Duration = Duration::from_millis(200);

/// Stride applied to the secondary picker counter. Coprime to the
/// primary stride (`1`) to ensure the two indices walk independent
/// orbits around the slot ring; any small odd integer that is not a
/// factor of common pool sizes (1, 2, 3, 4, 8, 14, 16) works.
const PICKER_STRIDE_B: usize = 7;

/// Builder for fresh [`Worker`] instances.
///
/// Used by the pool both at startup and for in-place replacement when
/// a worker is recycled. Returning an `Arc<dyn Worker>` lets the pool
/// publish workers via the per-slot mutex without re-boxing.
#[async_trait]
pub trait WorkerFactory: Send + Sync + 'static {
    /// Builds a fully booted [`Worker`] for the given pool slot.
    ///
    /// `worker_id` is forwarded to the engine's [`crate::ops::WorkerId`]
    /// so worker-aware ops (logging, console, etc.) can gate output.
    /// Slot index `0` is treated as the primary worker.
    ///
    /// # Errors
    ///
    /// [`WorkerError::Engine`] when the underlying construction
    /// fails (e.g. the JS entrypoint is invalid).
    async fn build(&self, worker_id: usize) -> Result<Arc<dyn Worker>, WorkerError>;
}

/// Production factory that produces [`IsolateWorker`]s rooted at a
/// fixed JavaScript entrypoint.
pub struct IsolateWorkerFactory {
    entrypoint: PathBuf,
    workers: usize,
}

impl IsolateWorkerFactory {
    /// Constructs a factory pinned to `entrypoint` and a known pool size.
    #[must_use]
    pub const fn new(entrypoint: PathBuf, workers: usize) -> Self {
        Self { entrypoint, workers }
    }
}

#[async_trait]
impl WorkerFactory for IsolateWorkerFactory {
    async fn build(&self, worker_id: usize) -> Result<Arc<dyn Worker>, WorkerError> {
        let worker =
            IsolateWorker::spawn(self.entrypoint.clone(), worker_id, self.workers).await?;
        Ok(Arc::new(worker))
    }
}

/// Aggregate telemetry returned by [`IsolatePool::stats`].
#[derive(Debug, Clone)]
pub struct PoolStats {
    /// Total number of successful dispatches across all workers.
    pub dispatch_count: usize,
    /// Total number of recycle events triggered by the policy.
    pub recycle_count: usize,
    /// Per-worker health snapshot.
    pub worker_health: Vec<WorkerHealth>,
    /// Per-slot inflight request count at the moment of observation.
    pub worker_inflight: Vec<usize>,
}

/// Per-slot state carried by [`PoolInner`].
struct Slot {
    /// Current worker handle. Locked only long enough to clone the
    /// `Arc` — a single atomic increment — so the lock is effectively
    /// wait-free for dispatchers. The lock is never held across an
    /// `await`, eliminating head-of-line blocking even when many
    /// tasks dispatch concurrently to the same slot.
    worker: Mutex<Arc<dyn Worker>>,
    /// Number of requests currently being processed by this slot.
    /// Maintained by the [`InflightGuard`] RAII type — incremented on
    /// dispatch entry, decremented on dispatch completion or
    /// cancellation.
    inflight: AtomicUsize,
    /// Singleflight guard preventing the background recycler from
    /// starting overlapping rebuilds for the same slot.
    recycling: AtomicBool,
}

impl Slot {
    fn new(worker: Arc<dyn Worker>) -> Self {
        Self {
            worker: Mutex::new(worker),
            inflight: AtomicUsize::new(0),
            recycling: AtomicBool::new(false),
        }
    }

    fn current_worker(&self) -> Arc<dyn Worker> {
        self.worker.lock().expect("slot mutex poisoned").clone()
    }

    fn replace_worker(&self, fresh: Arc<dyn Worker>) {
        *self.worker.lock().expect("slot mutex poisoned") = fresh;
    }
}

/// Shared pool state. Held behind `Arc<PoolInner>` so the background
/// recycler can keep a `Weak` reference and exit cleanly when the
/// pool is dropped.
struct PoolInner {
    slots: Vec<Slot>,
    picker_a: AtomicUsize,
    picker_b: AtomicUsize,
    dispatch_count: AtomicUsize,
    recycle_count: AtomicUsize,
    factory: Arc<dyn WorkerFactory>,
    policy: Arc<dyn RecyclePolicy>,
}

impl PoolInner {
    fn pick_index(&self) -> usize {
        let n = self.slots.len();
        if n == 1 {
            return 0;
        }
        let a = self.picker_a.fetch_add(1, Ordering::Relaxed) % n;
        let raw_b = self.picker_b.fetch_add(PICKER_STRIDE_B, Ordering::Relaxed) % n;
        let b = if raw_b == a { (raw_b + 1) % n } else { raw_b };
        let load_a = self.slots[a].inflight.load(Ordering::Relaxed);
        let load_b = self.slots[b].inflight.load(Ordering::Relaxed);
        if load_a <= load_b { a } else { b }
    }

    async fn dispatch(&self, request: ProtoRequest) -> Result<ResponsePayload, DispatchError> {
        let idx = self.pick_index();
        let worker = self.slots[idx].current_worker();
        let _guard = InflightGuard::new(&self.slots[idx].inflight);
        match worker.dispatch(Job { request }).await {
            Ok(payload) => {
                self.dispatch_count.fetch_add(1, Ordering::Relaxed);
                Ok(payload)
            }
            Err(err) => Err(map_worker_error(err)),
        }
    }

    async fn run_recycle_pass(&self) {
        for idx in 0..self.slots.len() {
            self.recycle_slot_if_needed(idx).await;
        }
    }

    async fn recycle_slot_if_needed(&self, idx: usize) {
        let slot = &self.slots[idx];
        let worker = slot.current_worker();
        let health = worker.health();
        if !self.policy.should_recycle(&health) {
            return;
        }
        if slot
            .recycling
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return;
        }
        let outcome = self.factory.build(idx).await;
        match outcome {
            Ok(fresh) => {
                slot.replace_worker(fresh);
                self.recycle_count.fetch_add(1, Ordering::Relaxed);
                tracing::info!(
                    worker = idx,
                    heap_used = health.heap.used_heap_size,
                    heap_limit = health.heap.heap_size_limit,
                    requests = health.requests_handled,
                    "worker recycled by background recycler"
                );
            }
            Err(err) => {
                tracing::error!(error = %err, worker = idx, "recycle build failed");
            }
        }
        slot.recycling.store(false, Ordering::Release);
    }
}

/// RAII counter for in-flight requests against a single slot.
struct InflightGuard<'a> {
    counter: &'a AtomicUsize,
}

impl<'a> InflightGuard<'a> {
    fn new(counter: &'a AtomicUsize) -> Self {
        counter.fetch_add(1, Ordering::Relaxed);
        Self { counter }
    }
}

impl Drop for InflightGuard<'_> {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Hybrid hot-isolate pool with policy-driven recycling.
///
/// The `IsolatePool` is the public façade. All shared state lives in
/// the inner [`PoolInner`] — held behind `Arc` — and the background
/// recycler task references the inner via [`Weak`] so the pool can be
/// dropped cleanly without lingering tasks.
pub struct IsolatePool {
    inner: Arc<PoolInner>,
    recycler: JoinHandle<()>,
}

impl IsolatePool {
    /// Builds a pool of `size` workers using `factory` and `policy`,
    /// and starts the background recycler task.
    ///
    /// Workers are constructed concurrently — each `build()` call
    /// spawns a dedicated isolate thread which boots in parallel with
    /// its peers. Wall-clock startup is therefore bounded by the
    /// slowest single worker, not the sum of all of them.
    ///
    /// # Errors
    ///
    /// [`WorkerError::Engine`] when any of the initial workers fails
    /// to boot. Already-spawned workers are dropped together with the
    /// failed result; the function fails fast on the first error
    /// encountered while joining.
    ///
    /// # Panics
    ///
    /// Panics when `size` is zero.
    pub async fn new(
        size: usize,
        factory: Arc<dyn WorkerFactory>,
        policy: Arc<dyn RecyclePolicy>,
    ) -> Result<Self, WorkerError> {
        assert!(size > 0, "IsolatePool requires at least one worker");
        let mut join_set = tokio::task::JoinSet::new();
        for index in 0..size {
            let factory = factory.clone();
            join_set.spawn(async move {
                let worker = factory.build(index).await?;
                Ok::<(usize, Arc<dyn Worker>), WorkerError>((index, worker))
            });
        }
        let mut placeholders: Vec<Option<Arc<dyn Worker>>> = (0..size).map(|_| None).collect();
        while let Some(joined) = join_set.join_next().await {
            let (index, worker) = match joined {
                Ok(Ok(pair)) => pair,
                Ok(Err(err)) => {
                    join_set.abort_all();
                    return Err(err);
                }
                Err(join_err) => {
                    join_set.abort_all();
                    return Err(WorkerError::Engine(format!(
                        "worker boot task failed: {join_err}"
                    )));
                }
            };
            placeholders[index] = Some(worker);
        }
        let slots = placeholders
            .into_iter()
            .map(|maybe| Slot::new(maybe.expect("every worker slot must be filled")))
            .collect();
        let inner = Arc::new(PoolInner {
            slots,
            picker_a: AtomicUsize::new(0),
            picker_b: AtomicUsize::new(0),
            dispatch_count: AtomicUsize::new(0),
            recycle_count: AtomicUsize::new(0),
            factory,
            policy,
        });
        let recycler = tokio::spawn(recycler_loop(Arc::downgrade(&inner)));
        Ok(Self { inner, recycler })
    }

    /// Convenience constructor that wires the production
    /// [`IsolateWorkerFactory`] with the recycle policy resolved from
    /// the environment.
    ///
    /// Recognised env vars (see also [`reap_heap_ratio_from_env`] and
    /// [`reap_request_count_from_env`] for the parsing contracts):
    ///
    /// * `NEXIDE_REAP_HEAP_RATIO` (default `0.8`) — recycle a worker
    ///   when its V8 used-heap exceeds this fraction of the heap
    ///   limit. Set to `0` to disable the heap-watchdog branch.
    /// * `NEXIDE_REAP_AFTER_REQUESTS` (default `50000`) — recycle a
    ///   worker after it has handled this many requests. Set to `0`
    ///   to disable the request-count branch.
    /// * `NEXIDE_REAP_HEAP_MB` (optional) — recycle a worker when
    ///   its V8 used-heap exceeds this absolute byte budget. Useful
    ///   when `NEXIDE_HEAP_LIMIT_MB` is shrunk and the ratio policy
    ///   is too coarse-grained.
    /// * `NEXIDE_REAP_RSS_MB` (optional, Linux only) — recycle when
    ///   process-wide RSS exceeds this cap. On non-Linux hosts the
    ///   sampler always returns `None` so the policy disables itself.
    ///
    /// Setting all to `0` produces a no-op recycler — useful for
    /// benchmarks where the recycle path itself is being measured.
    /// Invalid env values (typos, negatives, NaN) are surfaced via a
    /// `warn!` log and treated as "not set" so the defaults apply.
    ///
    /// # Errors
    ///
    /// See [`Self::new`].
    pub async fn with_isolate_workers(
        size: usize,
        entrypoint: PathBuf,
    ) -> Result<Self, WorkerError> {
        let factory = Arc::new(IsolateWorkerFactory::new(entrypoint, size));
        let heap_raw = std::env::var("NEXIDE_REAP_HEAP_RATIO").ok();
        let req_raw = std::env::var("NEXIDE_REAP_AFTER_REQUESTS").ok();
        let heap_mb_raw = std::env::var("NEXIDE_REAP_HEAP_MB").ok();
        let rss_mb_raw = std::env::var("NEXIDE_REAP_RSS_MB").ok();
        let heap_ratio = reap_heap_ratio_from_env(heap_raw.as_deref());
        let request_count = reap_request_count_from_env(req_raw.as_deref());
        let heap_bytes = super::recycle::reap_heap_bytes_from_env(heap_mb_raw.as_deref());
        let rss_bytes = super::recycle::reap_rss_bytes_from_env(rss_mb_raw.as_deref());
        if heap_raw.is_some() && heap_ratio.is_none() {
            tracing::warn!(
                value = ?heap_raw,
                "NEXIDE_REAP_HEAP_RATIO is set but unparseable — falling back to default"
            );
        }
        if req_raw.is_some() && request_count.is_none() {
            tracing::warn!(
                value = ?req_raw,
                "NEXIDE_REAP_AFTER_REQUESTS is set but unparseable — falling back to default"
            );
        }
        if heap_mb_raw.is_some() && heap_bytes.is_none() {
            tracing::warn!(
                value = ?heap_mb_raw,
                "NEXIDE_REAP_HEAP_MB is set but unparseable — ignoring"
            );
        }
        if rss_mb_raw.is_some() && rss_bytes.is_none() {
            tracing::warn!(
                value = ?rss_mb_raw,
                "NEXIDE_REAP_RSS_MB is set but unparseable — ignoring"
            );
        }
        let sampler = rss_bytes
            .filter(|n| *n > 0)
            .map(|_| super::mem_sampler::ProcessSampler::live());
        let policy = super::recycle::build_default_recycle_policy_with(
            heap_ratio,
            request_count,
            heap_bytes,
            rss_bytes,
            sampler,
        );
        Self::new(size, factory, policy).await
    }

    /// Returns aggregated telemetry (Query — pure, no side effects).
    #[must_use]
    pub fn stats(&self) -> PoolStats {
        let worker_health = self
            .inner
            .slots
            .iter()
            .map(|s| s.current_worker().health())
            .collect();
        let worker_inflight = self
            .inner
            .slots
            .iter()
            .map(|s| s.inflight.load(Ordering::Relaxed))
            .collect();
        PoolStats {
            dispatch_count: self.inner.dispatch_count.load(Ordering::Relaxed),
            recycle_count: self.inner.recycle_count.load(Ordering::Relaxed),
            worker_health,
            worker_inflight,
        }
    }

    /// Returns the configured pool size.
    #[must_use]
    pub fn size(&self) -> usize {
        self.inner.slots.len()
    }

    /// Synchronously runs exactly one recycle pass over every slot.
    ///
    /// Production traffic is served by a background task that wakes
    /// every [`RECYCLE_TICK`]; this method is the deterministic seam
    /// for tests and any caller that needs to force-recycle without
    /// waiting for the timer.
    pub async fn tick_recycler(&self) {
        self.inner.run_recycle_pass().await;
    }
}

impl Drop for IsolatePool {
    fn drop(&mut self) {
        self.recycler.abort();
    }
}

#[async_trait]
impl EngineDispatcher for IsolatePool {
    async fn dispatch(&self, request: ProtoRequest) -> Result<ResponsePayload, DispatchError> {
        self.inner.dispatch(request).await
    }

    fn dispatch_count(&self) -> usize {
        self.inner.dispatch_count.load(Ordering::Relaxed)
    }
}

/// Background recycler loop.
///
/// Runs while the [`Arc<PoolInner>`] stays alive. Each tick walks the
/// slot list and calls [`PoolInner::recycle_slot_if_needed`], which
/// is itself singleflight-guarded. The loop exits cleanly the moment
/// the [`Weak`] handle can no longer be upgraded — i.e. when the
/// owning [`IsolatePool`] is dropped.
async fn recycler_loop(weak: Weak<PoolInner>) {
    let mut interval = tokio::time::interval(RECYCLE_TICK);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    interval.tick().await;
    loop {
        interval.tick().await;
        let Some(inner) = weak.upgrade() else { return };
        inner.run_recycle_pass().await;
    }
}

fn map_worker_error(err: WorkerError) -> DispatchError {
    match err {
        WorkerError::Shutdown => DispatchError::WorkerGone,
        WorkerError::BadRequest(msg) | WorkerError::Engine(msg) => DispatchError::BodyRead(msg),
    }
}
