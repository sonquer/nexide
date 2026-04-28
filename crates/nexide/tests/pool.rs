//! Integration tests for [`IsolatePool`] using a deterministic
//! recording worker — no V8 involved, so the suite is fast and
//! deterministic.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

use async_trait::async_trait;
use bytes::Bytes;
use nexide::dispatch::{EngineDispatcher, ProtoRequest};
use nexide::engine::HeapStats;
use nexide::ops::{ResponseHead, ResponsePayload};
use nexide::pool::{
    IsolatePool, Job, RecyclePolicy, RequestCount, Worker, WorkerError, WorkerFactory,
    WorkerHealth,
};

/// Worker that records which instance handled each request. All
/// state uses interior mutability so the worker satisfies
/// `Worker::dispatch(&self, ...)`.
struct RecordingWorker {
    id: u32,
    handled: AtomicU64,
    heap_used: AtomicU64,
    heap_limit: usize,
}

#[async_trait]
impl Worker for RecordingWorker {
    async fn dispatch(&self, job: Job) -> Result<ResponsePayload, WorkerError> {
        let count = self.handled.fetch_add(1, Ordering::Relaxed) + 1;
        let bump = job.request.body.len().max(64) as u64;
        self.heap_used.fetch_add(bump, Ordering::Relaxed);
        Ok(ResponsePayload {
            head: ResponseHead {
                status: 200,
                headers: vec![("x-worker-id".into(), self.id.to_string())],
            },
            body: Bytes::from(format!("worker-{}:req-{}", self.id, count)),
        })
    }

    fn health(&self) -> WorkerHealth {
        let used = usize::try_from(self.heap_used.load(Ordering::Relaxed)).unwrap_or(usize::MAX);
        WorkerHealth {
            heap: HeapStats {
                used_heap_size: used,
                total_heap_size: used,
                heap_size_limit: self.heap_limit,
            },
            requests_handled: self.handled.load(Ordering::Relaxed),
        }
    }
}

/// Factory that hands out [`RecordingWorker`]s with monotonically
/// increasing IDs.
struct RecordingFactory {
    next_id: AtomicU32,
    heap_limit: usize,
    builds: Arc<AtomicUsize>,
}

impl RecordingFactory {
    fn new(heap_limit: usize) -> (Arc<Self>, Arc<AtomicUsize>) {
        let builds = Arc::new(AtomicUsize::new(0));
        (
            Arc::new(Self {
                next_id: AtomicU32::new(0),
                heap_limit,
                builds: builds.clone(),
            }),
            builds,
        )
    }
}

#[async_trait]
impl WorkerFactory for RecordingFactory {
    async fn build(&self, _worker_id: usize) -> Result<Arc<dyn Worker>, WorkerError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.builds.fetch_add(1, Ordering::Relaxed);
        Ok(Arc::new(RecordingWorker {
            id,
            handled: AtomicU64::new(0),
            heap_used: AtomicU64::new(0),
            heap_limit: self.heap_limit,
        }))
    }
}

/// Policy that never recycles — used to test pure routing behaviour
/// without recycle interference.
struct NeverRecycle;
impl RecyclePolicy for NeverRecycle {
    fn should_recycle(&self, _snapshot: &WorkerHealth) -> bool {
        false
    }
}

fn proto(uri: &str) -> ProtoRequest {
    ProtoRequest {
        method: "GET".to_owned(),
        uri: uri.to_owned(),
        headers: Vec::new(),
        body: Bytes::new(),
    }
}

fn body_string(payload: &ResponsePayload) -> String {
    String::from_utf8(payload.body.to_vec()).expect("utf8")
}

#[tokio::test]
async fn sequential_dispatch_round_robins_idle_workers() {
    let (factory, _builds) = RecordingFactory::new(1_000_000);
    let pool = IsolatePool::new(3, factory, Arc::new(NeverRecycle))
        .await
        .expect("pool");

    let mut bodies = Vec::new();
    for i in 0..6 {
        let payload = pool.dispatch(proto(&format!("/r{i}"))).await.expect("ok");
        bodies.push(body_string(&payload));
    }

    assert_eq!(
        bodies,
        vec![
            "worker-0:req-1",
            "worker-1:req-1",
            "worker-2:req-1",
            "worker-0:req-2",
            "worker-1:req-2",
            "worker-2:req-2",
        ],
        "with all slots idle the P2C picker degenerates to round-robin via picker_a",
    );
    assert_eq!(pool.dispatch_count(), 6);
}

#[tokio::test]
async fn concurrent_dispatch_balances_load_across_workers() {
    let (factory, _builds) = RecordingFactory::new(1_000_000);
    let pool = Arc::new(
        IsolatePool::new(4, factory, Arc::new(NeverRecycle))
            .await
            .expect("pool"),
    );

    let mut handles = Vec::new();
    for i in 0..200 {
        let pool = pool.clone();
        handles.push(tokio::spawn(async move {
            pool.dispatch(proto(&format!("/r{i}")))
                .await
                .expect("dispatch")
        }));
    }
    for h in handles {
        h.await.expect("join");
    }

    let stats = pool.stats();
    assert_eq!(stats.dispatch_count, 200);
    assert_eq!(stats.worker_health.len(), 4);
    let total: u64 = stats.worker_health.iter().map(|h| h.requests_handled).sum();
    assert_eq!(total, 200);
    for (idx, h) in stats.worker_health.iter().enumerate() {
        assert!(
            h.requests_handled > 0,
            "worker {idx} should receive traffic under uniform load"
        );
    }
}

#[tokio::test]
async fn background_recycler_replaces_overdue_workers_on_tick() {
    let (factory, builds) = RecordingFactory::new(1_000_000);
    let policy: Arc<dyn RecyclePolicy> = Arc::new(RequestCount::new(2));
    let pool = IsolatePool::new(1, factory, policy).await.expect("pool");
    assert_eq!(builds.load(Ordering::Relaxed), 1);

    let r1 = body_string(&pool.dispatch(proto("/a")).await.expect("ok"));
    let r2 = body_string(&pool.dispatch(proto("/b")).await.expect("ok"));
    assert_eq!(r1, "worker-0:req-1");
    assert_eq!(r2, "worker-0:req-2");

    pool.tick_recycler().await;

    let r3 = body_string(&pool.dispatch(proto("/c")).await.expect("ok"));
    assert_eq!(
        r3, "worker-1:req-1",
        "after the recycler tick the slot must be served by a fresh worker"
    );

    let stats = pool.stats();
    assert_eq!(stats.dispatch_count, 3);
    assert_eq!(stats.recycle_count, 1);
    assert_eq!(builds.load(Ordering::Relaxed), 2);
}

#[tokio::test]
async fn recycler_tick_is_singleflight_per_pass() {
    let (factory, builds) = RecordingFactory::new(1_000_000);
    let policy: Arc<dyn RecyclePolicy> = Arc::new(RequestCount::new(1));
    let pool = IsolatePool::new(2, factory, policy).await.expect("pool");

    for _ in 0..6 {
        pool.dispatch(proto("/")).await.expect("ok");
    }
    pool.tick_recycler().await;

    let stats = pool.stats();
    assert_eq!(stats.dispatch_count, 6);
    assert_eq!(
        stats.recycle_count, 2,
        "one tick rebuilds each overdue slot exactly once",
    );
    assert_eq!(builds.load(Ordering::Relaxed), 4);
    for h in &stats.worker_health {
        assert_eq!(h.requests_handled, 0, "freshly recycled workers");
    }
}

#[tokio::test]
async fn pool_stats_aggregates_worker_health() {
    let (factory, _builds) = RecordingFactory::new(1_000_000);
    let pool = IsolatePool::new(2, factory, Arc::new(NeverRecycle))
        .await
        .expect("pool");

    pool.dispatch(proto("/")).await.expect("ok");
    pool.dispatch(proto("/")).await.expect("ok");
    pool.dispatch(proto("/")).await.expect("ok");

    let stats = pool.stats();
    assert_eq!(stats.worker_health.len(), 2);
    assert_eq!(stats.worker_inflight.len(), 2);
    let total: u64 = stats.worker_health.iter().map(|h| h.requests_handled).sum();
    assert_eq!(total, 3);
    for live in &stats.worker_inflight {
        assert_eq!(*live, 0, "inflight returns to zero after dispatch completes");
    }
}
