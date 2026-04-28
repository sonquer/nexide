//! Docker-backed bench: each `(preset × runtime)` cell runs in its
//! own container with `--cpus` and `--memory` limits applied.
//!
//! Resource sampling is taken from the container's own cgroup (CPU
//! delta, memory `usage`) via the streaming Docker stats API, so the
//! numbers reflect what the workload sees inside the container —
//! exactly the cap we want to characterise.
//!
//! Image build is delegated to `docker build`: the orchestrator only
//! ensures the two named images exist before a sweep starts.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use bollard::Docker;
use bollard::models::{ContainerCreateBody, HostConfig, PortBinding};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, RemoveContainerOptionsBuilder,
    StartContainerOptions, StatsOptionsBuilder, StopContainerOptionsBuilder,
};
use futures::StreamExt;
use tokio::sync::Mutex;
use tokio::time::{Instant, sleep};
use tracing::{debug, info, warn};

use crate::load::{LoadSpec, run_load};
use crate::runner::{BenchResult, RouteSpec};
use crate::sample::SampleStats;
use crate::target::TargetKind;

/// Default image tags for the three runtimes. Built from the
/// Dockerfiles under `docker/` at the workspace root.
pub const DEFAULT_NEXIDE_IMAGE: &str = "nexide-bench/nexide:latest";
/// Default image tag for the Node.js runtime container.
pub const DEFAULT_NODE_IMAGE: &str = "nexide-bench/node:latest";
/// Default image tag for the Deno runtime container.
pub const DEFAULT_DENO_IMAGE: &str = "nexide-bench/deno:latest";

/// Resource cap applied to one bench cell.
#[derive(Debug, Clone, Copy)]
pub struct DockerPreset {
    /// Number of CPU cores allocated (`--cpus`).
    pub cpus: f64,
    /// Memory limit in mebibytes (`--memory`).
    pub memory_mb: u64,
}

impl DockerPreset {
    /// AWS Lambda min (1 cpu, 128 MiB).
    pub const MICRO: Self = Self { cpus: 1.0, memory_mb: 128 };
    /// Fly.io shared-cpu-1x (1 cpu, 256 MiB).
    pub const TINY: Self = Self { cpus: 1.0, memory_mb: 256 };
    /// Common entry tier (1 cpu, 512 MiB).
    pub const SMALL: Self = Self { cpus: 1.0, memory_mb: 512 };
    /// Cloud Run minimum (1 cpu, 1024 MiB).
    pub const MEDIUM: Self = Self { cpus: 1.0, memory_mb: 1024 };
    /// Lambda 1-cpu break point (1 cpu, 1769 MiB).
    pub const LAMBDA_1C: Self = Self { cpus: 1.0, memory_mb: 1769 };
    /// 2 cpu, 512 MiB — exposes CPU vs memory trade-off.
    pub const SMALL_2C: Self = Self { cpus: 2.0, memory_mb: 512 };
    /// Typical k8s pod (2 cpu, 1024 MiB).
    pub const MEDIUM_2C: Self = Self { cpus: 2.0, memory_mb: 1024 };
    /// Cloud Run default (2 cpu, 2048 MiB).
    pub const LARGE_2C: Self = Self { cpus: 2.0, memory_mb: 2048 };
    /// Container worker (4 cpu, 2048 MiB).
    pub const LARGE_4C: Self = Self { cpus: 4.0, memory_mb: 2048 };
    /// Dedicated host tier (4 cpu, 4096 MiB).
    pub const XLARGE_4C: Self = Self { cpus: 4.0, memory_mb: 4096 };

    /// Catalog of named presets in ascending resource order.
    pub const CATALOG: &'static [(&'static str, Self)] = &[
        ("1cpu-128mb", Self::MICRO),
        ("1cpu-256mb", Self::TINY),
        ("1cpu-512mb", Self::SMALL),
        ("1cpu-1024mb", Self::MEDIUM),
        ("1cpu-1769mb", Self::LAMBDA_1C),
        ("2cpu-512mb", Self::SMALL_2C),
        ("2cpu-1024mb", Self::MEDIUM_2C),
        ("2cpu-2048mb", Self::LARGE_2C),
        ("4cpu-2048mb", Self::LARGE_4C),
        ("4cpu-4096mb", Self::XLARGE_4C),
    ];

    /// Default sweep used when the user does not pass `--preset`.
    pub const DEFAULT_SWEEP: &'static [Self] = &[
        Self::TINY,
        Self::SMALL,
        Self::MEDIUM,
        Self::MEDIUM_2C,
    ];

    /// Parse a preset label (`<n>cpu-<m>mb`).
    ///
    /// # Errors
    /// Returns an error if the label does not match the grammar
    /// `<n>cpu-<m>mb` (n is parsed as `f64`, m as `u64`).
    pub fn parse(label: &str) -> Result<Self> {
        let (cpu_part, mem_part) = label
            .split_once('-')
            .with_context(|| format!("invalid preset '{label}': expected `<n>cpu-<m>mb`"))?;
        let cpus: f64 = cpu_part
            .strip_suffix("cpu")
            .with_context(|| format!("invalid cpu part: {cpu_part}"))?
            .parse()
            .context("cpu count")?;
        let memory_mb: u64 = mem_part
            .strip_suffix("mb")
            .with_context(|| format!("invalid mem part: {mem_part}"))?
            .parse()
            .context("memory mb")?;
        Ok(Self { cpus, memory_mb })
    }

    /// Canonical short label (`1cpu-512mb`).
    #[must_use]
    pub fn label(&self) -> String {
        let cpus_str = if (self.cpus.fract()).abs() < f64::EPSILON {
            format!("{}cpu", self.cpus as u64)
        } else {
            format!("{:.1}cpu", self.cpus)
        };
        format!("{cpus_str}-{}mb", self.memory_mb)
    }
}

/// Tags for the three images used in a docker-mode sweep.
#[derive(Debug, Clone)]
pub struct DockerImages {
    /// Image hosting `nexide` listening on guest port 3000.
    pub nexide_image: String,
    /// Image hosting `node server.js` listening on guest port 3000.
    pub node_image: String,
    /// Image hosting `deno run server.js` listening on guest port 3000.
    pub deno_image: String,
}

impl Default for DockerImages {
    fn default() -> Self {
        Self {
            nexide_image: DEFAULT_NEXIDE_IMAGE.to_owned(),
            node_image: DEFAULT_NODE_IMAGE.to_owned(),
            deno_image: DEFAULT_DENO_IMAGE.to_owned(),
        }
    }
}

/// Specification of a docker-mode bench sweep.
#[derive(Debug, Clone)]
pub struct DockerBench {
    /// Resource caps to sweep (sequentially).
    pub presets: Vec<DockerPreset>,
    /// Image tags for each runtime.
    pub images: DockerImages,
    /// Routes to drive.
    pub routes: Vec<RouteSpec>,
    /// Concurrent virtual users.
    pub connections: usize,
    /// Wall-clock measurement window per cell.
    pub duration: Duration,
    /// Warmup window per cell.
    pub warmup: Duration,
    /// Readiness timeout per container boot.
    pub ready_timeout: Duration,
    /// Runtimes to compare (defaults to nexide+node).
    pub runtimes: Vec<TargetKind>,
}

/// Output of a single preset run.
#[derive(Debug, Clone)]
pub struct DockerRun {
    /// Cap that produced these results.
    pub preset: DockerPreset,
    /// One result per `(route, runtime)` cell.
    pub results: Vec<BenchResult>,
}

const GUEST_PORT: u16 = 3000;

fn pick_host_port() -> Result<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .context("bind ephemeral host port")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

fn image_for(images: &DockerImages, kind: TargetKind) -> &str {
    match kind {
        TargetKind::Nexide => &images.nexide_image,
        TargetKind::Node => &images.node_image,
        TargetKind::Deno => &images.deno_image,
    }
}

/// Ensure all required runtime images exist locally; build any missing
/// one via `docker build` from the provided Dockerfile.
///
/// When `rebuild` is `true`, every image is rebuilt unconditionally
/// (the Docker layer cache may still apply, but the build is always
/// invoked). This is the escape hatch for "I changed Rust source but
/// the cached image won the race" — the most common foot-gun when
/// iterating on `nexide` performance fixes.
///
/// # Errors
/// Returns an error when `docker build` fails or when the workspace
/// root does not contain the expected `docker/` Dockerfiles.
pub async fn ensure_images(
    docker: &Docker,
    images: &DockerImages,
    workspace_root: &Path,
    rebuild: bool,
) -> Result<()> {
    for (image, dockerfile) in [
        (
            &images.nexide_image,
            "crates/nexide-bench/docker/nexide/Dockerfile",
        ),
        (
            &images.node_image,
            "crates/nexide-bench/docker/node/Dockerfile",
        ),
        (
            &images.deno_image,
            "crates/nexide-bench/docker/deno/Dockerfile",
        ),
    ] {
        if !rebuild && image_exists(docker, image).await? {
            debug!(%image, "image present, skipping build");
            continue;
        }
        let dockerfile_path = workspace_root.join(dockerfile);
        info!(%image, dockerfile=%dockerfile_path.display(), rebuild, "building image");
        let status = tokio::process::Command::new("docker")
            .arg("build")
            .arg("-f")
            .arg(&dockerfile_path)
            .arg("-t")
            .arg(image)
            .arg(workspace_root)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .await
            .with_context(|| format!("spawn docker build for {image}"))?;
        if !status.success() {
            bail!("docker build failed for {image} (exit {status})");
        }
    }
    Ok(())
}

async fn image_exists(docker: &Docker, image: &str) -> Result<bool> {
    match docker.inspect_image(image).await {
        Ok(_) => Ok(true),
        Err(bollard::errors::Error::DockerResponseServerError {
            status_code: 404, ..
        }) => Ok(false),
        Err(e) => Err(anyhow!(e).context("inspect image")),
    }
}

#[derive(Default, Clone)]
struct StatsAccumulator {
    cpu_samples: Vec<f64>,
    mem_samples_mb: Vec<f64>,
}

impl StatsAccumulator {
    fn finalize(self) -> SampleStats {
        let cpu_avg = mean(&self.cpu_samples);
        let cpu_max = self.cpu_samples.iter().copied().fold(0.0_f64, f64::max);
        let mem_avg_mb = mean(&self.mem_samples_mb);
        let mem_max_mb =
            self.mem_samples_mb.iter().copied().fold(0.0_f64, f64::max);
        SampleStats {
            cpu_avg,
            cpu_max,
            mem_avg_mb,
            mem_max_mb,
            threads_max: 0,
            samples: self.cpu_samples.len() as u64,
        }
    }
}

#[allow(clippy::cast_precision_loss)]
fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
}

#[allow(clippy::cast_precision_loss)]
fn cpu_percent(stat: &bollard::models::ContainerStatsResponse) -> Option<f64> {
    let cpu = stat.cpu_stats.as_ref()?;
    let pre = stat.precpu_stats.as_ref()?;
    let cpu_total = cpu.cpu_usage.as_ref()?.total_usage? as f64;
    let pre_total = pre.cpu_usage.as_ref()?.total_usage.unwrap_or(0) as f64;
    let sys = cpu.system_cpu_usage? as f64;
    let pre_sys = pre.system_cpu_usage.unwrap_or(0) as f64;
    let online = f64::from(cpu.online_cpus.unwrap_or(1));
    let cpu_delta = cpu_total - pre_total;
    let sys_delta = sys - pre_sys;
    if sys_delta <= 0.0 || cpu_delta < 0.0 {
        return None;
    }
    Some((cpu_delta / sys_delta) * online * 100.0)
}

#[allow(clippy::cast_precision_loss)]
fn mem_mb(stat: &bollard::models::ContainerStatsResponse) -> Option<f64> {
    let mem = stat.memory_stats.as_ref()?;
    Some(mem.usage? as f64 / 1024.0 / 1024.0)
}

async fn await_ready(host_port: u16, timeout: Duration) -> Result<()> {
    let url = format!("http://127.0.0.1:{host_port}/api/ping");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(500))
        .build()?;
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(resp) = client.get(&url).send().await
            && resp.status().is_success()
        {
            return Ok(());
        }
        sleep(Duration::from_millis(200)).await;
    }
    bail!("container at host:{host_port} never responded within {timeout:?}");
}

async fn warmup_route(url: &str, duration: Duration) {
    if duration.is_zero() {
        return;
    }
    let client = reqwest::Client::new();
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        let _ = client.get(url).send().await;
        sleep(Duration::from_millis(20)).await;
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn build_create_body(
    image: &str,
    kind: TargetKind,
    preset: DockerPreset,
    host_port: u16,
) -> ContainerCreateBody {
    let nano_cpus = (preset.cpus * 1_000_000_000.0) as i64;
    let memory_bytes = i64::try_from(preset.memory_mb * 1024 * 1024)
        .unwrap_or(i64::MAX);
    let port_key = format!("{GUEST_PORT}/tcp");
    let port_bindings = std::collections::HashMap::from([(
        port_key.clone(),
        Some(vec![PortBinding {
            host_ip: Some("127.0.0.1".to_owned()),
            host_port: Some(host_port.to_string()),
        }]),
    )]);
    let exposed_ports = vec![port_key];
    let env = match kind {
        TargetKind::Nexide => Some(vec![
            format!("NEXIDE_POOL_MEMORY_BUDGET_MB={}", preset.memory_mb),
            "HOSTNAME=0.0.0.0".to_owned(),
            "PORT=3000".to_owned(),
            "RUST_LOG=info".to_owned(),
        ]),
        TargetKind::Node => None,
        TargetKind::Deno => Some(vec!["DENO_NO_UPDATE_CHECK=1".to_owned()]),
    };
    ContainerCreateBody {
        image: Some(image.to_owned()),
        env,
        exposed_ports: Some(exposed_ports),
        host_config: Some(HostConfig {
            nano_cpus: Some(nano_cpus),
            memory: Some(memory_bytes),
            port_bindings: Some(port_bindings),
            auto_remove: Some(false),
            ..Default::default()
        }),
        ..Default::default()
    }
}

async fn run_cell(
    docker: Arc<Docker>,
    kind: TargetKind,
    image: &str,
    preset: DockerPreset,
    spec: &DockerBench,
) -> Result<Vec<BenchResult>> {
    let host_port = pick_host_port()?;
    let name = format!(
        "nexide-bench-{}-{}-{}",
        kind.label(),
        preset.label(),
        host_port
    );
    let body = build_create_body(image, kind, preset, host_port);
    let create_opts =
        CreateContainerOptionsBuilder::default().name(&name).build();
    let created = docker
        .create_container(Some(create_opts), body)
        .await
        .with_context(|| format!("create container {name}"))?;
    let id = created.id;

    let mut results = Vec::with_capacity(spec.routes.len());
    let inner = async {
        docker
            .start_container(&id, None::<StartContainerOptions>)
            .await
            .context("start container")?;
        await_ready(host_port, spec.ready_timeout).await?;

        for route in &spec.routes {
            let url = format!("http://127.0.0.1:{host_port}{}", route.path);
            warmup_route(&url, spec.warmup).await;

            let acc = Arc::new(Mutex::new(StatsAccumulator::default()));
            let stats_acc = Arc::clone(&acc);
            let stats_docker = Arc::clone(&docker);
            let stats_id = id.clone();
            let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel::<()>();
            let stats_task = tokio::spawn(async move {
                let opts = StatsOptionsBuilder::default().stream(true).build();
                let mut stream = stats_docker.stats(&stats_id, Some(opts));
                loop {
                    tokio::select! {
                        _ = &mut cancel_rx => break,
                        item = stream.next() => match item {
                            Some(Ok(stat)) => {
                                let cpu = cpu_percent(&stat);
                                let mem = mem_mb(&stat);
                                let mut guard = stats_acc.lock().await;
                                if let Some(c) = cpu { guard.cpu_samples.push(c); }
                                if let Some(m) = mem { guard.mem_samples_mb.push(m); }
                            }
                            Some(Err(err)) => {
                                warn!(%err, "stats stream error");
                                break;
                            }
                            None => break,
                        }
                    }
                }
            });

            let load = run_load(LoadSpec {
                url,
                connections: spec.connections,
                duration: spec.duration,
                headers: route.headers.clone(),
            })
            .await?;

            let _ = cancel_tx.send(());
            let _ = stats_task.await;
            let stats = Arc::try_unwrap(acc)
                .map(Mutex::into_inner)
                .unwrap_or_default()
                .finalize();
            results.push(BenchResult {
                route: route.id.clone(),
                runtime: kind,
                load,
                sample: stats,
            });
        }
        anyhow::Ok(())
    }
    .await;

    let stop_opts = StopContainerOptionsBuilder::default().t(2).build();
    let _ = docker.stop_container(&id, Some(stop_opts)).await;
    let rm_opts = RemoveContainerOptionsBuilder::default().force(true).build();
    let _ = docker.remove_container(&id, Some(rm_opts)).await;

    inner?;
    Ok(results)
}

/// Run the full docker-mode bench sweep.
///
/// Iterates `spec.presets` × `spec.runtimes` × `spec.routes` strictly
/// sequentially (one container at a time) so that resource caps are
/// not contaminated by cross-cell contention.
///
/// # Errors
/// Returns an error when the Docker daemon is unreachable, an image
/// is missing, or a container fails its readiness probe.
pub async fn run_docker(
    docker: &Arc<Docker>,
    spec: &DockerBench,
) -> Result<Vec<DockerRun>> {
    info!(
        presets = spec.presets.len(),
        runtimes = spec.runtimes.len(),
        routes = spec.routes.len(),
        "starting docker sweep"
    );
    let mut runs = Vec::with_capacity(spec.presets.len());
    for preset in &spec.presets {
        info!(preset = %preset.label(), "preset start");
        let mut cells = Vec::new();
        for kind in &spec.runtimes {
            let image = image_for(&spec.images, *kind);
            let cell =
                run_cell(Arc::clone(docker), *kind, image, *preset, spec)
                    .await
                    .with_context(|| {
                        format!(
                            "preset={} runtime={}",
                            preset.label(),
                            kind.label()
                        )
                    })?;
            cells.extend(cell);
        }
        runs.push(DockerRun {
            preset: *preset,
            results: cells,
        });
    }
    Ok(runs)
}

/// Connect to the local Docker daemon (Unix socket on Linux/macOS,
/// named pipe on Windows).
///
/// # Errors
/// Returns an error when the Docker daemon is not reachable.
pub fn connect_docker() -> Result<Docker> {
    Docker::connect_with_local_defaults()
        .context("connect to local docker daemon")
}

/// Locate the workspace root by walking up from `cwd` until a
/// `Cargo.toml` next to a `crates/` directory is found.
///
/// # Errors
/// Returns an error when no workspace root is found before reaching
/// the filesystem root.
pub fn detect_workspace_root() -> Result<PathBuf> {
    let mut cur = std::env::current_dir().context("read cwd")?;
    loop {
        if cur.join("Cargo.toml").is_file() && cur.join("crates").is_dir() {
            return Ok(cur);
        }
        if !cur.pop() {
            bail!("could not locate workspace root from cwd");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_parses_canonical_labels() {
        let p = DockerPreset::parse("1cpu-256mb").unwrap();
        assert!((p.cpus - 1.0).abs() < f64::EPSILON);
        assert_eq!(p.memory_mb, 256);
        let p = DockerPreset::parse("2cpu-1024mb").unwrap();
        assert!((p.cpus - 2.0).abs() < f64::EPSILON);
        assert_eq!(p.memory_mb, 1024);
        let p = DockerPreset::parse("0.5cpu-128mb").unwrap();
        assert!((p.cpus - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn preset_rejects_malformed_labels() {
        assert!(DockerPreset::parse("1cpu").is_err());
        assert!(DockerPreset::parse("xcpu-256mb").is_err());
        assert!(DockerPreset::parse("1cpu-").is_err());
    }

    #[test]
    fn preset_label_round_trip() {
        for (label, preset) in DockerPreset::CATALOG {
            assert_eq!(&preset.label(), label, "round-trip: {label}");
        }
    }

    #[test]
    fn catalog_is_ordered_by_total_resources() {
        for window in DockerPreset::CATALOG.windows(2) {
            let a = window[0].1;
            let b = window[1].1;
            let a_score = a.cpus.mul_add(4096.0, a.memory_mb as f64);
            let b_score = b.cpus.mul_add(4096.0, b.memory_mb as f64);
            assert!(
                a_score <= b_score,
                "catalog out of order: {} > {}",
                window[0].0,
                window[1].0
            );
        }
    }
}
