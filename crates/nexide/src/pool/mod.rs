//! Hot-isolate worker pool with policy-driven recycling.
//!
//! This is the production substrate that the HTTP shield talks to:
//! [`IsolatePool`] implements [`crate::dispatch::EngineDispatcher`]
//! and owns N [`Worker`]s, each pinned to its own OS thread plus a
//! dedicated V8 isolate. After every dispatch, the configured
//! [`RecyclePolicy`] decides whether the worker should be retired
//! and replaced with a freshly booted one.

mod engine_pump;
mod isolate_pool;
mod isolate_worker;
mod local_isolate_worker;
mod local_pool;
mod mem_sampler;
mod pump_strategy;
mod recycle;
mod worker;

pub use isolate_pool::{IsolatePool, IsolateWorkerFactory, PoolStats, WorkerFactory};
pub use isolate_worker::IsolateWorker;
pub use local_isolate_worker::LocalIsolateWorker;
pub use local_pool::LocalIsolatePool;
pub use mem_sampler::{MemorySampler, MemorySample, MockSampler, ProcessSampler};
pub use pump_strategy::{
    Coalesced, BoxedPumpStrategy, DEFAULT_BATCH, MAX_BATCH, Serial, PumpStrategy,
    pump_strategy_from_env,
};
pub use recycle::{
    Composite, HeapBytes, HeapThreshold, ProcessRss, RecyclePolicy, RequestCount,
    build_default_recycle_policy, build_default_recycle_policy_with, reap_heap_bytes_from_env,
    reap_heap_ratio_from_env, reap_request_count_from_env, reap_rss_bytes_from_env,
};
pub use worker::{Job, Worker, WorkerError, WorkerHealth};
