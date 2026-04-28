//! Top-level orchestration: for each route × each runtime, spawn the
//! target, sample it while [`run_load`](crate::run_load) is in flight,
//! and collect a [`BenchResult`] suitable for [`render_report`].

use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::time::sleep;

use crate::load::{LoadOutcome, LoadSpec, run_load};
use crate::sample::{ProcessSampler, SampleStats};
use crate::target::{TargetKind, spawn_target};

/// Single route to benchmark on each runtime.
#[derive(Debug, Clone)]
pub struct RouteSpec {
    /// Stable identifier shown in the report (e.g. `api-time`).
    pub id: String,
    /// HTTP path (e.g. `/api/time`).
    pub path: String,
    /// Extra headers to send with every request.
    pub headers: Vec<(String, String)>,
}

/// Top-level benchmark configuration.
#[derive(Debug, Clone)]
pub struct BenchConfig {
    /// Workspace root containing `target/release/nexide` and the
    /// example standalone bundle.
    pub workspace_root: PathBuf,
    /// Routes to benchmark.
    pub routes: Vec<RouteSpec>,
    /// Concurrent virtual users.
    pub connections: usize,
    /// Wall-clock duration of each measurement window.
    pub duration: Duration,
    /// Warmup window before sampling starts.
    pub warmup: Duration,
    /// Sampling interval for the CPU/RSS sidecar.
    pub sample_interval: Duration,
    /// Readiness timeout for each spawned target.
    pub ready_timeout: Duration,
    /// Runtimes to compare (defaults to nexide+node).
    pub runtimes: Vec<TargetKind>,
}

impl Default for BenchConfig {
    fn default() -> Self {
        Self {
            workspace_root: PathBuf::from("."),
            routes: default_routes(),
            connections: 64,
            duration: Duration::from_secs(30),
            warmup: Duration::from_secs(5),
            sample_interval: Duration::from_millis(250),
            ready_timeout: Duration::from_secs(30),
            runtimes: vec![TargetKind::Nexide, TargetKind::Node, TargetKind::Deno],
        }
    }
}

/// One row of the report: a `(route, runtime)` cell with full metrics.
#[derive(Debug, Clone)]
pub struct BenchResult {
    /// Which route was driven.
    pub route: String,
    /// Which runtime hosted it.
    pub runtime: TargetKind,
    /// Load-side metrics.
    pub load: LoadOutcome,
    /// Resource-side metrics.
    pub sample: SampleStats,
}

/// Default route set: small JSON, ping, prerendered HTML, RSC, native
/// `/_next/image` optimizer.
#[must_use]
pub fn default_routes() -> Vec<RouteSpec> {
    vec![
        RouteSpec {
            id: "api-time".into(),
            path: "/api/time".into(),
            headers: Vec::new(),
        },
        RouteSpec {
            id: "api-ping".into(),
            path: "/api/ping".into(),
            headers: Vec::new(),
        },
        RouteSpec {
            id: "ssg-about".into(),
            path: "/about".into(),
            headers: Vec::new(),
        },
        RouteSpec {
            id: "rsc-about".into(),
            path: "/about".into(),
            headers: vec![("RSC".into(), "1".into())],
        },
        RouteSpec {
            id: "next-image".into(),
            path: "/_next/image?url=%2Fnexide.png&w=256&q=75".into(),
            headers: vec![("Accept".into(), "image/webp,*/*".into())],
        },
    ]
}

fn pick_free_port() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").context("bind ephemeral port")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

async fn warmup(addr: SocketAddr, path: &str, duration: Duration) {
    if duration.is_zero() {
        return;
    }
    let url = format!("http://{addr}{path}");
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + duration;
    while tokio::time::Instant::now() < deadline {
        let _ = client.get(&url).send().await;
        sleep(Duration::from_millis(20)).await;
    }
}

async fn run_cell(
    cfg: &BenchConfig,
    runtime: TargetKind,
    route: &RouteSpec,
) -> Result<BenchResult> {
    let port = pick_free_port()?;
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse()?;
    let mut handle = spawn_target(runtime, addr, &cfg.workspace_root, cfg.ready_timeout).await?;
    warmup(addr, &route.path, cfg.warmup).await;
    let pid = handle.pid()?;
    let sampler = ProcessSampler::spawn(pid, cfg.sample_interval)?;
    let load = run_load(LoadSpec {
        url: format!("http://{addr}{}", route.path),
        connections: cfg.connections,
        duration: cfg.duration,
        headers: route.headers.clone(),
    })
    .await?;
    let sample = sampler.stop().await?;
    handle.shutdown().await?;
    Ok(BenchResult {
        route: route.id.clone(),
        runtime,
        load,
        sample,
    })
}

/// Run the full matrix and return one [`BenchResult`] per cell. Cells
/// are executed sequentially so they don't compete for CPU.
///
/// # Errors
/// Returns an error from the first failing cell; previously-completed
/// cells are still printed by the caller before the error propagates.
pub async fn run_bench(cfg: &BenchConfig) -> Result<Vec<BenchResult>> {
    ensure_artifacts(&cfg.workspace_root)?;
    let mut results = Vec::with_capacity(cfg.routes.len() * cfg.runtimes.len());
    for route in &cfg.routes {
        for runtime in &cfg.runtimes {
            tracing::info!(route = %route.id, runtime = runtime.label(), "benchmarking");
            results.push(run_cell(cfg, *runtime, route).await?);
        }
    }
    Ok(results)
}

fn ensure_artifacts(root: &Path) -> Result<()> {
    let bin = root.join("target/release/nexide");
    let bundle = root.join("example/.next/standalone/server.js");
    anyhow::ensure!(
        bin.is_file(),
        "missing nexide release binary: {} - run `cargo build --release`",
        bin.display()
    );
    anyhow::ensure!(
        bundle.is_file(),
        "missing standalone bundle: {} - run `npm run build` in example/",
        bundle.display()
    );
    Ok(())
}
