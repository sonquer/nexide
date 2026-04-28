//! Public surface for the nexide-bench harness. Re-exports the
//! orchestration entrypoints used by `main.rs` so they can also be
//! exercised from integration tests.

#![deny(missing_docs)]
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::missing_panics_doc
)]

pub mod docker;
pub mod load;
pub mod report;
pub mod runner;
pub mod sample;
pub mod target;

pub use docker::{
    DEFAULT_DENO_IMAGE, DEFAULT_NEXIDE_IMAGE, DEFAULT_NODE_IMAGE, DockerBench, DockerImages,
    DockerPreset, DockerRun, connect_docker, detect_workspace_root, ensure_images, run_docker,
};
pub use load::{LoadOutcome, LoadSpec, run_load};
pub use report::{
    SweepPoint, render_report, render_scaling_mem, render_scaling_p99, render_scaling_rps,
};
pub use runner::{BenchConfig, BenchResult, RouteSpec, run_bench};
pub use sample::{ProcessSampler, SampleStats};
pub use target::{TargetHandle, TargetKind, spawn_target};
