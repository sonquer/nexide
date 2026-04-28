//! `nexide-bench` — head-to-head benchmark CLI.
//!
//! Two execution modes:
//! - `docker` / `docker-suite`: drive the bench inside resource-capped
//!   containers built from the Dockerfiles shipped with this crate.
//!   This is the default for apples-to-apples deployment-shaped runs.
//! - `local` / `suite`: drive the same bundle on the host machine.
//!   Useful for bare-metal profiling and CI smoke tests.

#![deny(missing_docs)]
#![allow(clippy::print_stdout)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use nexide_bench::{
    BenchConfig, DockerBench, DockerImages, DockerPreset, RouteSpec, SweepPoint, TargetKind,
    connect_docker, ensure_images, render_report, render_scaling_mem, render_scaling_p99,
    render_scaling_rps, run_bench, run_docker,
};

#[derive(Debug, Parser)]
#[command(
    name = "nexide-bench",
    about = "Head-to-head benchmark: nexide vs node vs deno hosting Next.js standalone."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run a single docker-mode preset (default). Builds the
    /// `nexide-bench/nexide` and `nexide-bench/node` images on first
    /// use, then drives both inside `--cpus`/`--memory` capped
    /// containers.
    Docker(DockerArgs),
    /// Sweep multiple docker presets sequentially. Emits one
    /// abs/delta table per preset.
    DockerSuite(DockerSuiteArgs),
    /// Run the benchmark on the host machine (no containers).
    Local(LocalArgs),
    /// Sweep multiple concurrency levels on the host. Emits scaling
    /// matrices (RPS/p99/memory) at the end.
    Suite(SuiteArgs),
    /// List the named docker presets shipped with nexide-bench.
    Presets,
}

#[derive(Debug, Parser)]
struct DockerArgs {
    /// Workspace root (defaults to walking up from cwd).
    #[arg(long)]
    workspace_root: Option<PathBuf>,
    /// Resource preset, e.g. `1cpu-256mb`, `2cpu-1024mb`.
    #[arg(long, default_value = "1cpu-512mb")]
    preset: String,
    /// Override the image tag for the nexide container.
    #[arg(long)]
    nexide_image: Option<String>,
    /// Override the image tag for the node container.
    #[arg(long)]
    node_image: Option<String>,
    /// Override the image tag for the deno container.
    #[arg(long)]
    deno_image: Option<String>,
    /// Concurrent virtual users.
    #[arg(long, default_value_t = 64)]
    connections: usize,
    /// Measurement window per cell.
    #[arg(long, value_parser = parse_duration, default_value = "30s")]
    duration: Duration,
    /// Warmup window per cell.
    #[arg(long, value_parser = parse_duration, default_value = "5s")]
    warmup: Duration,
    /// Readiness timeout per container boot.
    #[arg(long, value_parser = parse_duration, default_value = "60s")]
    ready_timeout: Duration,
    /// Custom route definitions in `id=path[:HEADER=VALUE,...]` form.
    #[arg(long = "route")]
    routes: Vec<String>,
    /// Limit the run to one runtime.
    #[arg(long, value_parser = parse_runtime)]
    only: Option<TargetKind>,
    /// Force rebuild of all docker images, even if they already exist.
    /// Use after editing Rust source under `crates/` to defeat the
    /// stale-image trap.
    #[arg(long)]
    rebuild: bool,
}

#[derive(Debug, Parser)]
struct DockerSuiteArgs {
    /// Workspace root (defaults to walking up from cwd).
    #[arg(long)]
    workspace_root: Option<PathBuf>,
    /// Presets to sweep, comma-separated. Defaults to a balanced
    /// 4-step ladder.
    #[arg(
        long,
        value_delimiter = ',',
        num_args = 1..,
        default_values_t = vec![
            "1cpu-256mb".to_owned(),
            "1cpu-512mb".to_owned(),
            "1cpu-1024mb".to_owned(),
            "2cpu-1024mb".to_owned(),
        ]
    )]
    presets: Vec<String>,
    /// Override the image tag for the nexide container.
    #[arg(long)]
    nexide_image: Option<String>,
    /// Override the image tag for the node container.
    #[arg(long)]
    node_image: Option<String>,
    /// Override the image tag for the deno container.
    #[arg(long)]
    deno_image: Option<String>,
    /// Concurrent virtual users.
    #[arg(long, default_value_t = 64)]
    connections: usize,
    /// Measurement window per cell.
    #[arg(long, value_parser = parse_duration, default_value = "20s")]
    duration: Duration,
    /// Warmup window per cell.
    #[arg(long, value_parser = parse_duration, default_value = "3s")]
    warmup: Duration,
    /// Readiness timeout per container boot.
    #[arg(long, value_parser = parse_duration, default_value = "60s")]
    ready_timeout: Duration,
    /// Cool-down delay between consecutive presets.
    #[arg(long, value_parser = parse_duration, default_value = "2s")]
    cooldown: Duration,
    /// Custom route definitions in `id=path[:HEADER=VALUE,...]` form.
    #[arg(long = "route")]
    routes: Vec<String>,
    /// Limit the run to one runtime.
    #[arg(long, value_parser = parse_runtime)]
    only: Option<TargetKind>,
    /// Force rebuild of all docker images, even if they already exist.
    /// Use after editing Rust source under `crates/` to defeat the
    /// stale-image trap.
    #[arg(long)]
    rebuild: bool,
}

#[derive(Debug, Parser)]
struct SuiteArgs {
    #[arg(long)]
    workspace_root: Option<PathBuf>,
    #[arg(
        long,
        value_delimiter = ',',
        num_args = 1..,
        default_values_t = vec![16usize, 64, 256, 1024]
    )]
    connections: Vec<usize>,
    #[arg(long, value_parser = parse_duration, default_value = "20s")]
    duration: Duration,
    #[arg(long, value_parser = parse_duration, default_value = "3s")]
    warmup: Duration,
    #[arg(long, value_parser = parse_duration, default_value = "250ms")]
    sample_interval: Duration,
    #[arg(long, value_parser = parse_duration, default_value = "30s")]
    ready_timeout: Duration,
    #[arg(long, value_parser = parse_duration, default_value = "2s")]
    cooldown: Duration,
    #[arg(long = "route")]
    routes: Vec<String>,
    #[arg(long, value_parser = parse_runtime)]
    only: Option<TargetKind>,
}

#[derive(Debug, Parser)]
struct LocalArgs {
    #[arg(long)]
    workspace_root: Option<PathBuf>,
    #[arg(long, default_value_t = 64)]
    connections: usize,
    #[arg(long, value_parser = parse_duration, default_value = "30s")]
    duration: Duration,
    #[arg(long, value_parser = parse_duration, default_value = "5s")]
    warmup: Duration,
    #[arg(long, value_parser = parse_duration, default_value = "250ms")]
    sample_interval: Duration,
    #[arg(long, value_parser = parse_duration, default_value = "30s")]
    ready_timeout: Duration,
    #[arg(long = "route")]
    routes: Vec<String>,
    #[arg(long, value_parser = parse_runtime)]
    only: Option<TargetKind>,
}

fn parse_duration(s: &str) -> Result<Duration, String> {
    let t = s.trim();
    if let Some(rest) = t.strip_suffix("ms") {
        return rest
            .parse::<u64>()
            .map(Duration::from_millis)
            .map_err(|e| e.to_string());
    }
    if let Some(rest) = t.strip_suffix('s') {
        return rest
            .parse::<f64>()
            .map(Duration::from_secs_f64)
            .map_err(|e| e.to_string());
    }
    t.parse::<u64>()
        .map(Duration::from_secs)
        .map_err(|e| e.to_string())
}

fn parse_runtime(s: &str) -> Result<TargetKind, String> {
    match s.to_lowercase().as_str() {
        "nexide" => Ok(TargetKind::Nexide),
        "node" => Ok(TargetKind::Node),
        "deno" => Ok(TargetKind::Deno),
        other => Err(format!("unknown runtime: {other}")),
    }
}

fn parse_route(spec: &str) -> Result<RouteSpec> {
    let (id, rest) = spec
        .split_once('=')
        .with_context(|| format!("route '{spec}' must be id=path[:HEADER=VAL,...]"))?;
    let (path, headers_part) = match rest.split_once(':') {
        Some((p, h)) => (p, Some(h)),
        None => (rest, None),
    };
    let mut headers = Vec::new();
    if let Some(h) = headers_part {
        for kv in h.split(',') {
            let (k, v) = kv
                .split_once('=')
                .with_context(|| format!("header '{kv}' must be NAME=VALUE"))?;
            headers.push((k.trim().to_owned(), v.trim().to_owned()));
        }
    }
    Ok(RouteSpec {
        id: id.trim().to_owned(),
        path: path.trim().to_owned(),
        headers,
    })
}

fn detect_workspace_root() -> Result<PathBuf> {
    let mut cur = std::env::current_dir()?;
    loop {
        if cur.join("Cargo.toml").is_file() && cur.join("crates").is_dir() {
            return Ok(cur);
        }
        if !cur.pop() {
            bail!("could not locate workspace root from cwd");
        }
    }
}

fn resolve_routes(specs: &[String]) -> Result<Vec<RouteSpec>> {
    if specs.is_empty() {
        Ok(nexide_bench::runner::default_routes())
    } else {
        specs.iter().map(|s| parse_route(s)).collect()
    }
}

fn resolve_runtimes(only: Option<TargetKind>) -> Vec<TargetKind> {
    only.map_or_else(
        || vec![TargetKind::Nexide, TargetKind::Node, TargetKind::Deno],
        |only| vec![only],
    )
}

fn resolve_protagonist(runtimes: &[TargetKind]) -> TargetKind {
    if runtimes.contains(&TargetKind::Nexide) {
        TargetKind::Nexide
    } else {
        runtimes[0]
    }
}

fn build_images(
    nexide: Option<String>,
    node: Option<String>,
    deno: Option<String>,
) -> DockerImages {
    let mut images = DockerImages::default();
    if let Some(n) = nexide {
        images.nexide_image = n;
    }
    if let Some(n) = node {
        images.node_image = n;
    }
    if let Some(d) = deno {
        images.deno_image = d;
    }
    images
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let cli = Cli::parse();
    match cli.command {
        Command::Docker(args) => run_docker_one(args).await,
        Command::DockerSuite(args) => run_docker_suite(args).await,
        Command::Local(args) => run_local(args).await,
        Command::Suite(args) => run_suite(args).await,
        Command::Presets => {
            print_presets();
            Ok(())
        }
    }
}

fn print_presets() {
    println!("{:<16} {:>6} {:>10}", "label", "cpus", "memory MB");
    for (label, preset) in DockerPreset::CATALOG {
        println!("{:<16} {:>6} {:>10}", label, preset.cpus, preset.memory_mb,);
    }
}

async fn run_docker_one(args: DockerArgs) -> Result<()> {
    let workspace_root = match args.workspace_root {
        Some(p) => p,
        None => detect_workspace_root()?,
    };
    let preset = DockerPreset::parse(&args.preset)?;
    let routes = resolve_routes(&args.routes)?;
    let runtimes = resolve_runtimes(args.only);
    let protagonist = resolve_protagonist(&runtimes);
    let images = build_images(args.nexide_image, args.node_image, args.deno_image);
    let docker = Arc::new(connect_docker()?);
    println!(">>> nexide-bench docker: ensuring images");
    ensure_images(&docker, &images, &workspace_root, args.rebuild).await?;
    let spec = DockerBench {
        presets: vec![preset],
        images,
        routes,
        connections: args.connections,
        duration: args.duration,
        warmup: args.warmup,
        ready_timeout: args.ready_timeout,
        runtimes,
    };
    println!(
        ">>> nexide-bench docker: preset={} routes={} window={:?} conns={}",
        preset.label(),
        spec.routes.len(),
        spec.duration,
        spec.connections,
    );
    let runs = run_docker(&docker, &spec).await?;
    for run in &runs {
        println!("--- preset = {} ---", run.preset.label());
        println!("{}", render_report(&run.results, protagonist));
    }
    Ok(())
}

async fn run_docker_suite(args: DockerSuiteArgs) -> Result<()> {
    let workspace_root = match args.workspace_root {
        Some(p) => p,
        None => detect_workspace_root()?,
    };
    let presets: Vec<DockerPreset> = args
        .presets
        .iter()
        .map(|s| DockerPreset::parse(s))
        .collect::<Result<_>>()?;
    let routes = resolve_routes(&args.routes)?;
    let runtimes = resolve_runtimes(args.only);
    let protagonist = resolve_protagonist(&runtimes);
    let images = build_images(args.nexide_image, args.node_image, args.deno_image);
    let docker = Arc::new(connect_docker()?);
    println!(">>> nexide-bench docker-suite: ensuring images");
    ensure_images(&docker, &images, &workspace_root, args.rebuild).await?;
    println!(
        ">>> nexide-bench docker-suite: {} routes × {} runtimes × {} presets × {:?} window @ {} conns",
        routes.len(),
        runtimes.len(),
        presets.len(),
        args.duration,
        args.connections,
    );
    for (idx, preset) in presets.iter().copied().enumerate() {
        if idx > 0 && !args.cooldown.is_zero() {
            println!(">>> cooldown {:?}", args.cooldown);
            tokio::time::sleep(args.cooldown).await;
        }
        println!(
            ">>> [{}/{}] preset = {}",
            idx + 1,
            presets.len(),
            preset.label()
        );
        let spec = DockerBench {
            presets: vec![preset],
            images: images.clone(),
            routes: routes.clone(),
            connections: args.connections,
            duration: args.duration,
            warmup: args.warmup,
            ready_timeout: args.ready_timeout,
            runtimes: runtimes.clone(),
        };
        let runs = run_docker(&docker, &spec).await?;
        for run in &runs {
            println!("--- preset = {} ---", run.preset.label());
            println!("{}", render_report(&run.results, protagonist));
        }
    }
    Ok(())
}

async fn run_suite(args: SuiteArgs) -> Result<()> {
    let workspace_root = match args.workspace_root {
        Some(p) => p,
        None => detect_workspace_root()?,
    };
    let routes = resolve_routes(&args.routes)?;
    let runtimes = resolve_runtimes(args.only);
    let protagonist = resolve_protagonist(&runtimes);
    println!(
        ">>> nexide-bench suite: {} routes × {} runtimes × {} concurrency levels × {:?} window",
        routes.len(),
        runtimes.len(),
        args.connections.len(),
        args.duration,
    );
    let mut points: Vec<SweepPoint> = Vec::with_capacity(args.connections.len());
    for (idx, conns) in args.connections.iter().copied().enumerate() {
        if idx > 0 && !args.cooldown.is_zero() {
            println!(">>> cooldown {:?}", args.cooldown);
            tokio::time::sleep(args.cooldown).await;
        }
        println!(
            ">>> [{}/{}] sweep concurrency = {}",
            idx + 1,
            args.connections.len(),
            conns
        );
        let cfg = BenchConfig {
            workspace_root: workspace_root.clone(),
            routes: routes.clone(),
            connections: conns,
            duration: args.duration,
            warmup: args.warmup,
            sample_interval: args.sample_interval,
            ready_timeout: args.ready_timeout,
            runtimes: runtimes.clone(),
        };
        let results = run_bench(&cfg).await?;
        println!("--- concurrency = {conns} ---");
        println!("{}", render_report(&results, protagonist));
        points.push(SweepPoint {
            connections: conns,
            results,
        });
    }
    println!("{}", render_scaling_rps(&points));
    println!("{}", render_scaling_p99(&points));
    println!("{}", render_scaling_mem(&points));
    Ok(())
}

async fn run_local(args: LocalArgs) -> Result<()> {
    let workspace_root = match args.workspace_root {
        Some(p) => p,
        None => detect_workspace_root()?,
    };
    let routes = resolve_routes(&args.routes)?;
    let runtimes = resolve_runtimes(args.only);
    let protagonist = resolve_protagonist(&runtimes);
    let cfg = BenchConfig {
        workspace_root,
        routes,
        connections: args.connections,
        duration: args.duration,
        warmup: args.warmup,
        sample_interval: args.sample_interval,
        ready_timeout: args.ready_timeout,
        runtimes,
    };
    println!(
        ">>> nexide-bench local: {} routes × {} runtimes × {:?} window @ {} conns",
        cfg.routes.len(),
        cfg.runtimes.len(),
        cfg.duration,
        cfg.connections,
    );
    let results = run_bench(&cfg).await?;
    println!("{}", render_report(&results, protagonist));
    Ok(())
}
