//! Crate `nexide` — native Next.js runtime in Rust.
//!
//! The binary entrypoint defers to [`run`]. The [`serve_until`]
//! function exposes a testable seam that lets callers inject their
//! own shutdown future (the binary uses `tokio::signal::ctrl_c`).
//!
//! The [`server`] module exposes the HTTP shield: an Axum router that
//! pairs the static layer with a pluggable dynamic handler.

use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use thiserror::Error;
use tracing_subscriber::EnvFilter;

pub mod cli;
pub mod dispatch;
pub mod engine;
pub mod entrypoint;
pub mod ops;
pub mod pool;
pub mod server;

use self::cli::{BuildArgs, Cli, Command, DevArgs, StartArgs};
use self::entrypoint::{
    EntrypointKind, EntrypointResolver, ProductionEntrypointResolver, ResolvedEntrypoint,
};
use self::server::{ServerConfig, ServerError};
use clap::Parser;

/// Errors returned from the runtime entrypoint.
#[derive(Debug, Error)]
pub enum RuntimeError {
    /// The Tokio runtime could not be built.
    #[error("tokio runtime initialization failed: {0}")]
    Tokio(#[source] std::io::Error),

    /// The `tracing` subscriber could not be built.
    #[error("tracing subscriber initialization failed: {0}")]
    Tracing(String),

    /// The HTTP shield reported an error.
    #[error("http server failed: {0}")]
    Server(#[from] ServerError),

    /// The shield could not be configured.
    #[error("invalid server configuration: {0}")]
    Config(#[from] crate::server::ConfigError),

    /// A required directory from the production layout is missing.
    #[error("required directory missing: {0}")]
    MissingDir(PathBuf),

    /// A required file from the production layout is missing.
    #[error("required file missing: {0}")]
    MissingFile(PathBuf),

    /// The isolate pool could not be booted.
    #[error("isolate pool boot failed: {0}")]
    Pool(#[source] crate::pool::WorkerError),

    /// A delegated child process (`next dev` / `next build`) could
    /// not be launched.
    #[error("failed to launch `{program}`: {source}")]
    SpawnFailed {
        /// Program name we attempted to invoke.
        program: String,
        /// Underlying I/O error from the OS.
        #[source]
        source: std::io::Error,
    },

    /// A delegated child process exited with a non-zero status.
    #[error("`{program}` exited with status {status}")]
    DelegateFailed {
        /// Program name that exited.
        program: String,
        /// Stringified status (POSIX exit code or signal).
        status: String,
    },

    /// The user passed an invalid `--hostname` / `--port` combination.
    #[error("invalid bind address `{raw}`: {source}")]
    InvalidBind {
        /// Raw `host:port` string that failed to parse.
        raw: String,
        /// Underlying parser error.
        #[source]
        source: std::net::AddrParseError,
    },
}

/// Installs a global `tracing` subscriber driven by the `RUST_LOG`
/// environment variable. The function is idempotent — additional calls
/// inside the same process are no-ops.
///
/// # Errors
/// Returns [`RuntimeError::Tracing`] if the filter cannot be parsed
/// from the current environment configuration.
pub fn install_tracing() -> Result<(), RuntimeError> {
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new("info"))
        .map_err(|err| RuntimeError::Tracing(err.to_string()))?;
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .try_init();
    Ok(())
}

/// Runs the `nexide` runtime until `shutdown` resolves.
///
/// The function is the testability seam: production [`run`] passes
/// `tokio::signal::ctrl_c`, while tests can inject any future
/// (including one that resolves immediately).
///
/// # Errors
/// Returns [`RuntimeError`] propagated from [`install_tracing`] or
/// from the [`server::serve`] loop.
/// Serves the production runtime using the per-worker fast-path
/// architecture.
///
/// Spawns `worker_count` [`server::WorkerRuntime`] instances, each
/// hosting its own `current_thread` Tokio runtime, its own V8
/// isolate (via [`LocalIsolatePool`]) and its own copy of the Axum
/// router. The main multi-thread reactor binds the shared listener
/// and runs the [`server::accept_loop::run_accept_loop`] picker that
/// distributes incoming TCP connections across workers via adaptive
/// power-of-two-choices over their mailbox depth.
///
/// `worker_count` is clamped to `≥ 1`. Dispatch on each connection
/// stays intra-thread end-to-end — Axum, prerender, and the V8
/// isolate share the same `LocalSet` — eliminating the cross-thread
/// futex hop that historically dominated p99 latency on `--cpus=1`
/// and `--cpus=2` containers.
///
/// The function is the testability seam: production [`run`] passes
/// `tokio::signal::ctrl_c`, while tests can inject any future
/// (including one that resolves immediately).
///
/// # Errors
/// Returns [`RuntimeError`] propagated from [`install_tracing`] or
/// from [`server::serve_with_workers`].
pub async fn serve_until<F>(shutdown: F) -> Result<(), RuntimeError>
where
    F: Future<Output = ()> + Send + 'static,
{
    let cwd = std::env::current_dir().map_err(RuntimeError::Tokio)?;
    let layout = AppLayout::resolve(&cwd.join(EXAMPLE_ROOT), resolve_default_bind()?)?;
    serve_app_until(layout, shutdown).await
}

/// Filesystem layout of a built Next.js project plus the bind address
/// the runtime should listen on.
///
/// All paths are absolute and verified to exist at construction time
/// — downstream code can therefore consume them without further I/O
/// validation. Construct via [`AppLayout::resolve`].
#[derive(Debug, Clone)]
pub struct AppLayout {
    /// Resolved [`ServerConfig`] (paths + bind address) consumed by
    /// the Axum shield.
    pub config: ServerConfig,
    /// Resolved entrypoint script descriptor.
    pub entrypoint: ResolvedEntrypoint,
}

impl AppLayout {
    /// Resolves a Next.js standalone layout under `root` and binds it
    /// to the supplied `SocketAddr`.
    ///
    /// Two on-disk shapes are accepted, in this order:
    ///
    /// 1. **Project root** (`root` is the directory that holds
    ///    `package.json`). The resolver expects:
    ///    - `<root>/public/`
    ///    - `<root>/.next/static/`
    ///    - `<root>/.next/standalone/server.js`
    ///    - `<root>/.next/standalone/.next/server/app/`
    ///
    /// 2. **Standalone directory** (`root` is the deploy artefact, i.e.
    ///    the contents of `.next/standalone/`). Selected when
    ///    `<root>/server.js` exists. The resolver then expects:
    ///    - `<root>/server.js`
    ///    - `<root>/public/` (you must copy it from the project root —
    ///      `next build` does NOT copy `public/` and `.next/static/`
    ///      into the standalone bundle)
    ///    - `<root>/.next/static/` (likewise, copied manually)
    ///    - `<root>/.next/server/app/`
    ///
    /// # Errors
    /// Returns [`RuntimeError::MissingDir`] / [`RuntimeError::MissingFile`]
    /// when any required path is absent. The error message points at
    /// the missing path so the operator can fix the deploy layout.
    pub fn resolve(root: &Path, bind: SocketAddr) -> Result<Self, RuntimeError> {
        if root.join("server.js").is_file() {
            return Self::resolve_standalone_dir(root, bind);
        }
        Self::resolve_project_root(root, bind)
    }

    fn resolve_project_root(root: &Path, bind: SocketAddr) -> Result<Self, RuntimeError> {
        let public_dir = absolute_existing_dir(root, "public")?;
        let next_static_dir = absolute_existing_dir(root, ".next/static")?;
        let app_dir = absolute_existing_dir(root, ".next/standalone/.next/server/app")?;
        let config = ServerConfig::try_new(bind, public_dir, next_static_dir, app_dir)?;
        let resolver = ProductionEntrypointResolver::new(root);
        let entrypoint = resolver
            .resolve()
            .ok_or_else(|| RuntimeError::MissingFile(resolver.standalone_path().to_path_buf()))?;
        Ok(Self { config, entrypoint })
    }

    fn resolve_standalone_dir(root: &Path, bind: SocketAddr) -> Result<Self, RuntimeError> {
        let public_dir = absolute_existing_dir(root, "public")?;
        let next_static_dir = absolute_existing_dir(root, ".next/static")?;
        let app_dir = absolute_existing_dir(root, ".next/server/app")?;
        let config = ServerConfig::try_new(bind, public_dir, next_static_dir, app_dir)?;
        let entrypoint = ResolvedEntrypoint {
            path: root.join("server.js"),
            kind: EntrypointKind::NextStandalone,
        };
        Ok(Self { config, entrypoint })
    }
}

/// Drives the production runtime against an already-resolved
/// [`AppLayout`] until `shutdown` resolves.
///
/// This is the seam used by both the binary's `start` subcommand and
/// integration tests — they construct an [`AppLayout`] explicitly
/// instead of relying on the legacy `example/` discovery in
/// [`serve_until`].
///
/// # Errors
/// Propagates [`RuntimeError`] from [`install_tracing`] or from the
/// per-worker server loop.
pub async fn serve_app_until<F>(layout: AppLayout, shutdown: F) -> Result<(), RuntimeError>
where
    F: Future<Output = ()> + Send + 'static,
{
    install_tracing()?;
    let AppLayout { config, entrypoint } = layout;
    tracing::info!(
        kind = entrypoint.kind.label(),
        path = %entrypoint.path.display(),
        "loading entrypoint"
    );
    let worker_count = match detect_runtime_mode() {
        RuntimeMode::SingleThread => 1,
        RuntimeMode::MultiThread => default_pool_size(),
    };
    apply_v8_flags(worker_count);
    tracing::info!(
        bind = %config.bind(),
        workers = worker_count,
        "nexide runtime started"
    );
    let outcome =
        server::serve_with_workers(config, entrypoint.path, worker_count, shutdown).await;
    tracing::info!("nexide runtime stopped");
    outcome.map_err(RuntimeError::from)
}

/// Runtime threading topology selected at boot.
///
/// `MultiThread` reproduces the historical `nexide` model — the Axum
/// reactor lives on a multi-thread Tokio runtime and every V8 isolate
/// owns its own dedicated OS thread. It scales horizontally on
/// machines with two or more cores.
///
/// `SingleThread` collapses Axum **and** the V8 isolate onto the same
/// `current_thread` runtime + `LocalSet`, eliminating the
/// cross-thread futex hop that dominates p99 latency on `--cpus=1`
/// containers (see `docs/PERF_NOTES.md` Iteracja 2/3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeMode {
    /// Single OS thread hosts both the HTTP shield and the V8 isolate.
    SingleThread,
    /// Multi-threaded reactor with isolates pinned to dedicated worker threads.
    MultiThread,
}

impl RuntimeMode {
    /// Stable label used in tracing output and tests.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::SingleThread => "single-thread",
            Self::MultiThread => "multi-thread",
        }
    }
}

/// Environment variable that pins the runtime mode regardless of the
/// host CPU count.
///
/// Recognised values (case-insensitive, trimmed):
/// * `single`, `single-thread`, `1` → [`RuntimeMode::SingleThread`].
/// * `multi`, `multi-thread`, `multi_thread` → [`RuntimeMode::MultiThread`].
/// * `auto`, empty, missing → defer to [`std::thread::available_parallelism`].
///
/// Anything else is logged as a `warn!` and treated as `auto` so a
/// typo never silently changes deployment behaviour.
pub const RUNTIME_MODE_ENV: &str = "NEXIDE_RUNTIME_MODE";

/// Resolves the effective runtime mode for the current process.
///
/// Resolution order:
/// 1. [`RUNTIME_MODE_ENV`] explicit override.
/// 2. [`std::thread::available_parallelism`] — `1` ⇒ `SingleThread`,
///    anything else ⇒ `MultiThread`.
///
/// On exotic targets where parallelism cannot be detected, defaults
/// to [`RuntimeMode::MultiThread`] (the historically-tested path).
#[must_use]
pub fn detect_runtime_mode() -> RuntimeMode {
    resolve_runtime_mode(
        std::env::var(RUNTIME_MODE_ENV).ok().as_deref(),
        std::thread::available_parallelism()
            .map(std::num::NonZeroUsize::get)
            .ok(),
    )
}

/// Pure form of [`detect_runtime_mode`] for tests and reproducible
/// resolution.
#[must_use]
pub fn resolve_runtime_mode(env_value: Option<&str>, available_cpus: Option<usize>) -> RuntimeMode {
    if let Some(raw) = env_value.map(str::trim).filter(|s| !s.is_empty()) {
        match raw.to_ascii_lowercase().as_str() {
            "single" | "single-thread" | "single_thread" | "1" => {
                return RuntimeMode::SingleThread;
            }
            "multi" | "multi-thread" | "multi_thread" => return RuntimeMode::MultiThread,
            "auto" => {}
            other => {
                tracing::warn!(
                    value = %other,
                    "{RUNTIME_MODE_ENV}: unknown value, falling back to auto detection"
                );
            }
        }
    }
    match available_cpus {
        Some(1) => RuntimeMode::SingleThread,
        _ => RuntimeMode::MultiThread,
    }
}

/// Resolution order:
/// 1. `NEXIDE_POOL_SIZE` environment variable, if set to a positive
///    integer (e.g. `NEXIDE_POOL_SIZE=16`). Lets operators tune the
///    pool to deployment-specific traffic patterns without rebuilding
///    the binary. **Always wins** — bypasses every heuristic below.
/// 2. `NEXIDE_POOL_MEMORY_BUDGET_MB` (paired with
///    `NEXIDE_HEAP_PER_ISOLATE_MB`, default
///    [`DEFAULT_HEAP_PER_ISOLATE_MB`]): caps the pool to
///    `(budget_mb − recycle_reserve_mb) / per_isolate_mb`. The
///    recycle reserve is exactly one isolate's worth, because the
///    recycler boots a fresh isolate **before** swapping out the old
///    one — peak RSS during a swap is `(N+1) × per_isolate`, so the
///    sizing must subtract one isolate or the operator-supplied
///    budget is silently broken at the worst possible moment.
///    The result is then capped by the same CPU-based heuristic
///    used in (4)/(5) so a generous memory budget on a small box
///    does not spawn a thread bomb.
/// 3. Linux cgroup memory limit (v2 `memory.max`, fallback v1
///    `memory.limit_in_bytes`). When the runtime is deployed inside
///    a container with `--memory=…`, this gives the same answer as
///    (2) without forcing the operator to mirror the limit into env.
///    Skipped when the cgroup reports "no limit" (sentinel `max` /
///    `9223372036854771712`) because that indicates we are on the
///    host or in an unconstrained namespace, and the CPU-based
///    heuristic is the better signal there.
/// 4. Performance-core count minus a fixed headroom on heterogeneous
///    CPUs (Apple Silicon P/E split). Empirically derived from
///    `scripts/bench_pool_sweep.sh` on M3 Max (10 P-cores, 4 E-cores):
///    `POOL=8` wins on RPS (20.1k) and tail latency (p99=8.4ms),
///    `POOL=10` (raw P-core count) regresses to RPS=17.7k / p99=12.8ms
///    because each isolate drags V8 platform threads, the tokio worker
///    pool, the blocking pool and tower-http through a shared CPU
///    budget — saturating all P-cores forces work onto E-cores and
///    triples GC jitter. Reserving two P-cores for the rest of the
///    process keeps every isolate pinned to a P-core under load.
///    See [`pool_size_from_perf_cores`].
/// 5. [`std::thread::available_parallelism`], which honours
///    container CPU quotas (cgroup v1 `cpu.cfs_quota_us` and cgroup
///    v2 `cpu.max`) on Linux since Rust 1.59 — so a Pod limited to
///    `2` CPUs gets a 2-worker pool instead of 16-on-the-host.
/// 6. Hard fallback of `2` if every signal fails (exotic targets).
fn default_pool_size() -> usize {
    if let Some(explicit) = pool_size_from_env(std::env::var("NEXIDE_POOL_SIZE").ok().as_deref()) {
        return explicit;
    }
    let cpu_cap = cpu_based_pool_cap();
    if let Some(from_budget) = pool_size_from_memory_budget_env() {
        let capped = from_budget.min(cpu_cap);
        if capped < from_budget {
            tracing::info!(
                requested = from_budget,
                capped = capped,
                "pool size derived from NEXIDE_POOL_MEMORY_BUDGET_MB clamped by CPU heuristic"
            );
        }
        return capped;
    }
    if let Some(from_cgroup) = pool_size_from_cgroup_memory() {
        let capped = from_cgroup.min(cpu_cap);
        tracing::info!(
            from_cgroup,
            cpu_cap,
            chosen = capped,
            "pool size derived from cgroup memory limit"
        );
        return capped;
    }
    cpu_cap
}

/// Computes the CPU-based pool cap (steps 3–4–5 in the resolution
/// order documented on [`default_pool_size`]).
fn cpu_based_pool_cap() -> usize {
    perf_core_count().map_or_else(detected_pool_size, pool_size_from_perf_cores)
}

/// Reads the memory-budget envs and computes the pool-size suggestion.
///
/// Returns `None` if `NEXIDE_POOL_MEMORY_BUDGET_MB` is unset, blank or
/// invalid (a `warn!` is logged in the invalid case so misconfigured
/// deployments are not silent). The companion env
/// `NEXIDE_HEAP_PER_ISOLATE_MB` (default
/// [`DEFAULT_HEAP_PER_ISOLATE_MB`]) tunes the assumed steady-state
/// RSS per isolate; `128` MiB is the empirical observation from
/// `scripts/bench.sh` on the production Next.js bundle.
fn pool_size_from_memory_budget_env() -> Option<usize> {
    let budget_raw = std::env::var("NEXIDE_POOL_MEMORY_BUDGET_MB").ok();
    let per_iso_raw = std::env::var("NEXIDE_HEAP_PER_ISOLATE_MB").ok();
    let budget_mb = mb_from_env(budget_raw.as_deref());
    if budget_raw.is_some() && budget_mb.is_none() {
        tracing::warn!(
            value = ?budget_raw,
            "NEXIDE_POOL_MEMORY_BUDGET_MB is set but unparseable — ignoring"
        );
    }
    let budget_mb = budget_mb?;
    let per_iso_mb = mb_from_env(per_iso_raw.as_deref()).unwrap_or(DEFAULT_HEAP_PER_ISOLATE_MB);
    Some(pool_size_from_memory_budget(budget_mb, per_iso_mb))
}

/// Derives a pool size from a memory budget and a per-isolate heap
/// estimate, reserving one isolate's worth of headroom for the recycle
/// peak (the recycler boots a fresh isolate **before** retiring the
/// outgoing one).
///
/// Returns at least `1` to keep the runtime live even when the budget
/// is below `2 × per_isolate`; in that degenerate case a `warn!` is
/// logged from [`pool_size_from_memory_budget_env`] so the operator
/// learns the configured budget was not honoured. `per_isolate_mb`
/// is treated as `1` if the operator sets it to `0` to avoid a
/// division-by-zero.
fn pool_size_from_memory_budget(budget_mb: u64, per_isolate_mb: u64) -> usize {
    let per_iso = per_isolate_mb.max(1);
    let reserve = per_iso;
    let usable = budget_mb.saturating_sub(reserve);
    let raw = usable / per_iso;
    if raw == 0 {
        tracing::warn!(
            budget_mb,
            per_isolate_mb = per_iso,
            reserve_mb = reserve,
            "NEXIDE_POOL_MEMORY_BUDGET_MB cannot satisfy a single isolate plus recycle headroom; \
             starting with 1 worker — actual RSS will exceed the requested budget"
        );
        return 1;
    }
    usize::try_from(raw).unwrap_or(usize::MAX).max(1)
}

/// Reads the active cgroup memory limit (Linux only) and turns it
/// into a pool-size suggestion using the same arithmetic as
/// [`pool_size_from_memory_budget`]. Returns `None` on non-Linux
/// hosts, when the limit cannot be read, or when the limit is the
/// kernel "unlimited" sentinel — in those cases the caller falls
/// back to the CPU-based heuristic.
fn pool_size_from_cgroup_memory() -> Option<usize> {
    let budget_mb = cgroup_memory_limit_mb()?;
    let per_iso_mb = mb_from_env(std::env::var("NEXIDE_HEAP_PER_ISOLATE_MB").ok().as_deref())
        .unwrap_or(DEFAULT_HEAP_PER_ISOLATE_MB);
    Some(pool_size_from_memory_budget(budget_mb, per_iso_mb))
}

/// Cgroup memory limit reader (Linux): returns the limit in MiB, or
/// `None` when there is no real limit (host or unconstrained
/// namespace).
///
/// Implementation tries cgroup v2 (`/sys/fs/cgroup/memory.max`)
/// first, then v1 (`/sys/fs/cgroup/memory/memory.limit_in_bytes`).
/// On v2 the literal string `max` denotes "no limit"; on v1 the
/// kernel reports a near-`u64::MAX` sentinel
/// (`0x7FFF_FFFF_FFFF_F000`) for the same condition. Anything
/// `≥ 1 GiB × 1024` is treated as "host-scale" and ignored, so the
/// CPU heuristic remains in charge on bare-metal where the cgroup
/// file exists but exposes the host total.
#[cfg(target_os = "linux")]
fn cgroup_memory_limit_mb() -> Option<u64> {
    const HOST_SCALE_THRESHOLD_MB: u64 = 1024 * 1024;
    let raw_bytes = read_cgroup_v2_memory_max()
        .or_else(read_cgroup_v1_memory_limit)?;
    let mb = raw_bytes / (1024 * 1024);
    if mb < 1 || mb >= HOST_SCALE_THRESHOLD_MB {
        return None;
    }
    Some(mb)
}

/// Non-Linux stub: cgroups are a Linux concept, so on macOS / other
/// targets we always defer to the CPU-based heuristic.
#[cfg(not(target_os = "linux"))]
const fn cgroup_memory_limit_mb() -> Option<u64> {
    None
}

#[cfg(target_os = "linux")]
fn read_cgroup_v2_memory_max() -> Option<u64> {
    let raw = std::fs::read_to_string("/sys/fs/cgroup/memory.max").ok()?;
    let trimmed = raw.trim();
    if trimmed == "max" {
        return None;
    }
    trimmed.parse::<u64>().ok()
}

#[cfg(target_os = "linux")]
fn read_cgroup_v1_memory_limit() -> Option<u64> {
    let raw =
        std::fs::read_to_string("/sys/fs/cgroup/memory/memory.limit_in_bytes")
            .ok()?;
    let value = raw.trim().parse::<u64>().ok()?;
    if value >= 0x7FFF_FFFF_FFFF_F000 {
        return None;
    }
    Some(value)
}


/// RSS each isolate consumes when serving the production Next.js
/// standalone bundle under sustained load.
///
/// Reduced from a previous conservative `128` MiB after the
/// `nexide-bench docker-suite` run revealed that idle isolates settle
/// at ~50 MiB RSS and active ones at ~70 MiB; `64` is the rounded
/// midpoint that lets a 256 MiB container host 3 workers (vs 1
/// before) without OOM, and a 512 MiB container host 7 (vs 3).
const DEFAULT_HEAP_PER_ISOLATE_MB: u64 = 64;

/// Default cap on concurrent in-flight HTTP requests per V8 isolate.
///
/// The dispatcher acquires a permit from a per-isolate semaphore
/// before pushing a request into the [`crate::ops::DispatchTable`];
/// the permit is released when the JS handler settles (response or
/// error). Sixteen is the rubber-duck-validated starting point: high
/// enough to fully overlap async I/O for a Next.js handler under
/// real load, low enough that V8's microtask queue and the per-id
/// op map don't pathologically grow under burst.
///
/// Operators can override via [`MAX_INFLIGHT_PER_ISOLATE_ENV`]
/// (`NEXIDE_MAX_INFLIGHT_PER_ISOLATE`); values below `1` are clamped
/// to `1` so the runtime always remains live.
pub const DEFAULT_MAX_INFLIGHT_PER_ISOLATE: u32 = 16;

/// Environment variable that overrides
/// [`DEFAULT_MAX_INFLIGHT_PER_ISOLATE`].
pub const MAX_INFLIGHT_PER_ISOLATE_ENV: &str = "NEXIDE_MAX_INFLIGHT_PER_ISOLATE";

/// Resolves the effective in-flight cap by consulting the env, the
/// default, and clamping to `1..=u32::MAX`.
///
/// Returns the parsed value when [`MAX_INFLIGHT_PER_ISOLATE_ENV`]
/// holds a non-zero unsigned integer, otherwise
/// [`DEFAULT_MAX_INFLIGHT_PER_ISOLATE`]. A `warn!` is emitted when
/// the variable is set but unparseable so misconfigured deployments
/// are not silent. Pure function — environment access is the
/// caller's responsibility, which keeps unit tests deterministic.
#[must_use]
pub fn resolve_max_inflight_per_isolate(env_value: Option<&str>) -> u32 {
    match env_value.map(str::trim) {
        None | Some("") => DEFAULT_MAX_INFLIGHT_PER_ISOLATE,
        Some(raw) => match raw.parse::<u32>() {
            Ok(0) => {
                tracing::warn!(value = %raw, "{MAX_INFLIGHT_PER_ISOLATE_ENV}=0 clamped to 1");
                1
            }
            Ok(value) => value,
            Err(error) => {
                tracing::warn!(
                    value = %raw,
                    %error,
                    "{MAX_INFLIGHT_PER_ISOLATE_ENV} is unparseable — falling back to default"
                );
                DEFAULT_MAX_INFLIGHT_PER_ISOLATE
            }
        },
    }
}

/// Convenience wrapper that reads
/// [`MAX_INFLIGHT_PER_ISOLATE_ENV`] from the process environment.
///
/// Resolves the raw env value through
/// [`resolve_max_inflight_per_isolate`] so production call sites
/// don't need the pure form (tests use the pure form for
/// determinism).
#[must_use]
pub fn max_inflight_per_isolate() -> u32 {
    resolve_max_inflight_per_isolate(std::env::var(MAX_INFLIGHT_PER_ISOLATE_ENV).ok().as_deref())
}

/// Parses an unsigned integer "MB" env value.
fn mb_from_env(raw: Option<&str>) -> Option<u64> {
    raw.map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|n| *n >= 1)
}

/// Maps a raw performance-core count to an effective isolate-pool size
/// that leaves CPU headroom for the rest of the runtime (tokio workers,
/// blocking pool, tower-http, V8 platform threads).
///
/// The rule is `max(p - 2, 4)` for `p >= 6`, otherwise `p` verbatim —
/// guaranteeing at least one isolate, never overshooting the perf-core
/// budget and never shrinking pools below `4` on machines that have
/// the cores to support them.
fn pool_size_from_perf_cores(p: usize) -> usize {
    if p >= 6 { (p - 2).max(4) } else { p.max(1) }
}

/// Parses `NEXIDE_POOL_SIZE` content. Returns `None` for missing,
/// blank, non-numeric or zero values so the caller can fall back to
/// host detection.
fn pool_size_from_env(raw: Option<&str>) -> Option<usize> {
    raw.map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|n| *n >= 1)
}

/// Reads the host (or cgroup-constrained) parallelism, falling back
/// to `2` if the platform does not expose a value.
fn detected_pool_size() -> usize {
    std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(2)
}

/// Returns the number of "performance" CPU cores when the platform
/// exposes a heterogeneous topology (P-cores / E-cores), otherwise
/// `None`.
///
/// * **macOS / iOS**: queries `hw.perflevel0.physicalcpu` via
///   `sysctlbyname` (Apple Silicon exposes this; Intel Macs do not).
/// * **Other platforms**: returns `None`. Linux x86 hyperthreading is
///   handled implicitly because `available_parallelism()` already
///   reflects logical cores, which over-counts here; future work can
///   parse `/sys/devices/system/cpu/...` for big.LITTLE ARM.
#[cfg(target_vendor = "apple")]
fn perf_core_count() -> Option<usize> {
    use std::ffi::CString;
    use std::mem::size_of;
    let key = CString::new("hw.perflevel0.physicalcpu").ok()?;
    let mut value: i32 = 0;
    let mut size = size_of::<i32>();
    // SAFETY: `key` is a valid NUL-terminated C string for the lifetime of
    // this call; `value`/`size` point to a stack-allocated `i32` matching
    // the size advertised in `size`. `sysctlbyname` reads only `key.len()`
    // bytes from `key` and writes at most `size` bytes into `value`, both
    // of which are honoured. No FFI invariant is violated on failure.
    let rc = unsafe {
        libc::sysctlbyname(
            key.as_ptr(),
            std::ptr::from_mut::<i32>(&mut value).cast::<libc::c_void>(),
            &raw mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 || value <= 0 {
        return None;
    }
    usize::try_from(value).ok()
}

/// Non-Apple stub — see the macOS implementation for rationale.
#[cfg(not(target_vendor = "apple"))]
fn perf_core_count() -> Option<usize> {
    None
}

/// Synchronous binary entrypoint.
///
/// Builds a multi-threaded Tokio runtime to host the accept loop and
/// runs the shield until `ctrl-c`. Each [`server::WorkerRuntime`]
/// owns its own dedicated OS thread + `current_thread` runtime + V8
/// isolate (see [`serve_until`]).
///
/// Sizing rationale for the main reactor:
///
/// * `new_current_thread()` — the main process does
///   no per-request work. It only handles `tokio::signal::ctrl_c`,
///   spawns N worker threads, and (on non-Linux) runs the lightweight
///   p2c `accept_loop` which is bounded by `accept(2)` syscall rate
///   and trivial atomic stride bookkeeping. Reducing the reactor
///   from 2 worker threads to 1 removes one OS thread that would
///   otherwise compete with the per-worker isolates for CPU on small
///   containers (`--cpus=2` deployments are the worst case — see
///   `docs/PERF_NOTES.md` for the empirical 2cpu/1024 p99 regression that
///   motivated this change).
/// * `max_blocking_threads(rt_blocking_cap())` — caps the blocking
///   pool at `2 × cpus` (overridable via `NEXIDE_BLOCKING_THREADS`).
///   Tokio's default of `512` lets bursty `spawn_blocking` traffic
///   explode the OS thread count under load, starving the scheduler
///   and inflating tail latency. Each per-worker runtime has its own
///   blocking pool, so this cap only governs the main reactor's
///   blocking work (logging, signal-handling helpers).
/// * `thread_name("nexide-rt")` — distinguishes the reactor thread
///   from the per-worker `nexide-worker-N` threads and the V8
///   `DefaultWorker` pool when profiling with `sample`/`ps -M`.
///
/// # Errors
/// See [`RuntimeError`].
pub fn run() -> Result<(), RuntimeError> {
    let cli = Cli::parse();
    match cli.command {
        Command::Start(args) => run_start(args),
        Command::Dev(args) => run_dev(args),
        Command::Build(args) => run_build(args),
    }
}

/// Runs the production runtime against a project directory.
///
/// Builds a `current_thread` Tokio reactor (as documented for
/// [`run`]), resolves the [`AppLayout`] under `args.dir`, and serves
/// until SIGINT.
fn run_start(args: StartArgs) -> Result<(), RuntimeError> {
    let mode = detect_runtime_mode();
    tracing::info!(mode = mode.label(), "selected runtime mode");
    let bind = parse_bind(&args.hostname, args.port)?;
    let root = absolute_dir(&args.dir)?;
    let layout = AppLayout::resolve(&root, bind)?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(rt_blocking_cap())
        .thread_name("nexide-rt")
        .enable_all()
        .build()
        .map_err(RuntimeError::Tokio)?;
    rt.block_on(serve_app_until(layout, wait_for_ctrl_c()))
}

/// Delegates to `next dev` from the project's `node_modules/.bin/next`.
fn run_dev(args: DevArgs) -> Result<(), RuntimeError> {
    install_tracing()?;
    let root = absolute_dir(&args.dir)?;
    let mut argv: Vec<String> = vec![
        "dev".to_string(),
        "--port".to_string(),
        args.port.to_string(),
        "--hostname".to_string(),
        args.hostname.clone(),
    ];
    if args.turbo {
        argv.push("--turbo".to_string());
    }
    argv.push(root.display().to_string());
    delegate_to_next(&root, &argv)
}

/// Delegates to `next build` from the project's `node_modules/.bin/next`.
fn run_build(args: BuildArgs) -> Result<(), RuntimeError> {
    install_tracing()?;
    let root = absolute_dir(&args.dir)?;
    delegate_to_next(&root, &["build".to_string(), root.display().to_string()])
}

/// Locates `next` (project-local first, then `npx` fallback) and runs
/// it synchronously inside `cwd`. Forwards the child's exit status.
fn delegate_to_next(cwd: &Path, argv: &[String]) -> Result<(), RuntimeError> {
    let local_bin = cwd.join("node_modules/.bin/next");
    let (program, prefix_args) = if local_bin.is_file() {
        (local_bin.display().to_string(), Vec::<String>::new())
    } else {
        ("npx".to_string(), vec!["--no-install".to_string(), "next".to_string()])
    };
    let mut command = std::process::Command::new(&program);
    command.current_dir(cwd);
    command.args(&prefix_args);
    command.args(argv);
    tracing::info!(program = %program, args = ?argv, cwd = %cwd.display(), "delegating to next");
    let status = command
        .status()
        .map_err(|source| RuntimeError::SpawnFailed { program: program.clone(), source })?;
    if status.success() {
        return Ok(());
    }
    Err(RuntimeError::DelegateFailed {
        program,
        status: status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "terminated by signal".to_string()),
    })
}

/// Parses a CLI `host:port` pair into a [`SocketAddr`]. Hostnames
/// that are not IP literals fall back to `127.0.0.1` for IPv4 and the
/// unspecified address (`0.0.0.0`) when the user passes the magic
/// string `0.0.0.0` (matching `next start`).
fn parse_bind(hostname: &str, port: u16) -> Result<SocketAddr, RuntimeError> {
    let raw = if hostname.contains(':') && !hostname.starts_with('[') {
        format!("[{hostname}]:{port}")
    } else {
        format!("{hostname}:{port}")
    };
    raw.parse()
        .map_err(|source| RuntimeError::InvalidBind { raw, source })
}

/// Resolves and canonicalises a directory passed on the CLI.
fn absolute_dir(raw: &Path) -> Result<PathBuf, RuntimeError> {
    let cwd = std::env::current_dir().map_err(RuntimeError::Tokio)?;
    let candidate = if raw.is_absolute() { raw.to_path_buf() } else { cwd.join(raw) };
    if !candidate.is_dir() {
        return Err(RuntimeError::MissingDir(candidate));
    }
    Ok(candidate)
}

/// Resolves the upper bound for the main runtime's `spawn_blocking`
/// pool.
///
/// Resolution order:
/// 1. `NEXIDE_BLOCKING_THREADS` env var if set to a positive integer.
/// 2. `2 × available_parallelism` (one in-flight syscall per core
///    plus one queued, which matches the empirical sweet spot for
///    file-read-bound axum workloads).
/// 3. Hard fallback of `4` if both signals fail.
fn rt_blocking_cap() -> usize {
    blocking_cap_from_env(std::env::var("NEXIDE_BLOCKING_THREADS").ok().as_deref())
        .unwrap_or_else(detected_blocking_cap)
}

/// Parses `NEXIDE_BLOCKING_THREADS` content. Returns `None` for
/// missing, blank, non-numeric, or zero values so the caller can fall
/// back to host detection.
fn blocking_cap_from_env(raw: Option<&str>) -> Option<usize> {
    raw.map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|n| *n >= 1)
}

/// `2 × available_parallelism`, falling back to `4` on exotic targets
/// where parallelism cannot be detected.
fn detected_blocking_cap() -> usize {
    std::thread::available_parallelism()
        .map(std::num::NonZero::get)
        .map(|n| n.saturating_mul(2))
        .unwrap_or(4)
}

const DEFAULT_BIND: &str = "127.0.0.1:3000";
const BIND_ENV: &str = "NEXIDE_BIND";
const EXAMPLE_ROOT: &str = "example";

/// Returns the bind address from `NEXIDE_BIND`, falling back to
/// [`DEFAULT_BIND`] when the env var is absent or unparseable.
///
/// Used only by the legacy [`serve_until`] entry — the CLI path
/// builds its bind address from `--hostname` / `--port`.
fn resolve_default_bind() -> Result<SocketAddr, RuntimeError> {
    let raw = std::env::var(BIND_ENV).unwrap_or_else(|_| DEFAULT_BIND.to_owned());
    raw.parse()
        .map_err(|source| RuntimeError::InvalidBind { raw, source })
}

/// Joins `rel` onto `root`, returning [`RuntimeError::MissingDir`] if
/// the result does not exist on disk.
fn absolute_existing_dir(root: &Path, rel: &str) -> Result<PathBuf, RuntimeError> {
    let candidate = root.join(rel);
    if !candidate.is_dir() {
        return Err(RuntimeError::MissingDir(candidate));
    }
    Ok(candidate)
}

async fn wait_for_ctrl_c() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::error!(%error, "failed to listen for ctrl-c");
    }
}

/// Environment variable that injects V8 process-wide flags
/// before the first isolate boots.
///
/// When set to a non-empty string the value is treated as a **full
/// override** — it replaces [`DEFAULT_V8_FLAGS`] entirely so an
/// operator who knows what they are doing can dial in any flag combo
/// (e.g. `--predictable`, `--no-opt`, larger heap caps) without
/// inheriting the runtime's defaults. Set to an empty string to keep
/// the defaults; unset to do the same.
const V8_FLAGS_ENV: &str = "NEXIDE_V8_FLAGS";

/// Default V8 young-generation cap shared by every preset.
///
/// Matches V8's stock semi-space cap; documented here so the field is
/// not silently inherited from the V8 default and so changes show up
/// in source control.
const DEFAULT_SEMI_SPACE_CAP_MB: u64 = 16;

/// Per-worker safety margin reserved on top of the V8 old-generation
/// cap to leave headroom for non-V8 allocations (Rust-side `BytesMut`,
/// mpsc buffers, isolate metadata).
///
/// Empirically ~64 MiB is enough for the steady-state working set
/// outside V8 on a saturated worker.
const NON_V8_SAFETY_MB: u64 = 64;

/// Lower bound for the V8 old-generation cap when one is computed
/// from a container budget. Below this V8 cannot finish booting the
/// snapshot and `CommonJS` module graph reliably.
const MIN_OLD_SPACE_CAP_MB: u64 = 96;

/// Upper bound for the adaptive V8 old-generation cap.
///
/// phase A scales the cap with the per-worker memory share,
/// but on big presets (1cpu/1024 → 960 MiB cap; 2cpu/1024 → 448 MiB)
/// V8 lazily grows the heap close to the cap and major mark-sweep
/// runs become long enough to dominate p99 tail latency. The
/// The `docker-suite` measurements showed this directly:
/// p99 on `api-*` jumped from 64 ms (1cpu/512, cap 448) to 71 ms
/// (1cpu/1024, cap 960) and 49 ms (2cpu/1024, cap 448) — V8's
/// working set was hugging the cap and triggering long full-GC
/// pauses.
///
/// `256 MiB` is large enough to hold the production Next.js
/// working set with comfortable headroom (empirically ~150 MiB
/// hot) yet small enough that a worst-case full mark-sweep still
/// fits in single-digit milliseconds.
///
/// phase A turns this into the *floor* of an adaptive
/// ceiling: when `raw / 2 > HARD_OLD_SPACE_CAP_MB` we let V8 keep
/// the larger half so generously-provisioned containers
/// (e.g. 1cpu/1024 MiB) regain JIT-cache headroom. Tight presets
/// (1cpu/256, 2cpu/1024) still snap to this constant, which is
/// where the historical tail-latency win lives.
const HARD_OLD_SPACE_CAP_MB: u64 = 256;

/// Composes the default V8 flag string adaptively from the container
/// memory budget and the worker count.
///
/// * No budget → no `--max-old-space-size` (V8 grows lazily, which
///   keeps GC pressure low on dev machines that don't expose a
///   container memory limit).
/// * Budget present → cap each isolate at
///   `clamp(raw, MIN_OLD_SPACE_CAP_MB, max(HARD_OLD_SPACE_CAP_MB, raw/2))`
///   where `raw = budget/workers - NON_V8_SAFETY_MB`. The ceiling
///   adaptively loosens once the per-worker share is more than
///   2× the hard floor, so generously-provisioned containers
///   (e.g. 1cpu/1024 MiB) regain JIT-cache headroom without
///   re-introducing major-GC tail latency on tight presets
///   (phase A; supersedes the historical hard 256 MiB clamp).
///
/// `--max-semi-space-size=N` (with `N` from [`DEFAULT_SEMI_SPACE_CAP_MB`])
/// is always set so the young generation never grows faster than V8's
/// default — keeps minor GC pause bounded even when the old-generation
/// cap is large.
///
/// Pure function so the tuning policy is unit-testable without
/// mutating V8's process-global flag table.
fn compose_default_v8_flags(budget_mb: Option<u64>, workers: usize) -> String {
    use std::fmt::Write as _;
    let workers = workers.max(1) as u64;
    let mut flags = format!("--max-semi-space-size={DEFAULT_SEMI_SPACE_CAP_MB}");
    if let Some(budget) = budget_mb {
        let raw = budget.saturating_div(workers).saturating_sub(NON_V8_SAFETY_MB);
        let ceiling = HARD_OLD_SPACE_CAP_MB.max(raw / 2);
        let cap = raw.clamp(MIN_OLD_SPACE_CAP_MB, ceiling);
        let _ = write!(flags, " --max-old-space-size={cap}");
    }
    flags
}

/// Applies the V8 process-wide flags before any isolate is created.
///
/// Must be called before the first [`pool::IsolatePool`] boot —
/// once `v8::V8::initialize` runs (lazily on the first
/// `JsRuntime::try_new`), flags become read-only. The function is
/// idempotent (guarded by a `Once`).
fn apply_v8_flags(workers: usize) {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let flags = resolve_v8_flags(
            std::env::var(V8_FLAGS_ENV).ok().as_deref(),
            cgroup_memory_limit_mb(),
            workers,
        );
        tracing::info!(%flags, "applying V8 flags");
        v8::V8::set_flags_from_string(&flags);
    });
}

/// Picks the effective V8 flag string given the raw env value and the
/// detected container budget.
///
/// Returns the operator-supplied override when [`V8_FLAGS_ENV`] is
/// set to a non-blank value (full replacement of the computed
/// defaults), otherwise returns [`compose_default_v8_flags`] applied
/// to `budget_mb` and `workers`. Pure function so the resolution
/// logic is unit-testable without touching V8's process-global state.
fn resolve_v8_flags(
    env_value: Option<&str>,
    budget_mb: Option<u64>,
    workers: usize,
) -> String {
    env_value
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map_or_else(
            || compose_default_v8_flags(budget_mb, workers),
            str::to_owned,
        )
}

#[cfg(test)]
mod tests {
    use super::{
        AppLayout, BIND_ENV, DEFAULT_BIND, DEFAULT_HEAP_PER_ISOLATE_MB,
        DEFAULT_MAX_INFLIGHT_PER_ISOLATE, HARD_OLD_SPACE_CAP_MB, MIN_OLD_SPACE_CAP_MB,
        RUNTIME_MODE_ENV, RuntimeError, RuntimeMode, absolute_dir, blocking_cap_from_env,
        compose_default_v8_flags, detected_blocking_cap, detected_pool_size, install_tracing,
        mb_from_env, parse_bind, pool_size_from_env, pool_size_from_memory_budget,
        pool_size_from_perf_cores, resolve_default_bind, resolve_max_inflight_per_isolate,
        resolve_runtime_mode, resolve_v8_flags,
    };

    #[test]
    fn runtime_error_display_uses_stable_prefixes() {
        let tracing_err = RuntimeError::Tracing("boom".into());
        assert!(
            tracing_err.to_string().starts_with("tracing subscriber"),
            "unexpected: {tracing_err}"
        );

        let io = std::io::Error::other("disk gone");
        let tokio_err = RuntimeError::Tokio(io);
        assert!(
            tokio_err.to_string().starts_with("tokio runtime"),
            "unexpected: {tokio_err}"
        );
    }

    #[test]
    fn install_tracing_is_idempotent() {
        install_tracing().expect("first install");
        install_tracing().expect("second install");
    }

    #[test]
    fn resolve_bind_falls_back_to_default_when_env_missing() {
        // SAFETY: tests run sequentially in this module; manipulating
        // process-global env is acceptable for the duration of the test.
        unsafe { std::env::remove_var(BIND_ENV) };
        let addr = resolve_default_bind().expect("bind");
        assert_eq!(addr.to_string(), DEFAULT_BIND);
    }

    #[test]
    fn resolve_bind_honors_env_override() {
        unsafe { std::env::set_var(BIND_ENV, "127.0.0.1:54321") };
        let addr = resolve_default_bind().expect("bind");
        assert_eq!(addr.port(), 54321);
        unsafe { std::env::remove_var(BIND_ENV) };
    }

    #[test]
    fn resolve_bind_returns_error_for_unparseable_value() {
        unsafe { std::env::set_var(BIND_ENV, "not an address") };
        assert!(resolve_default_bind().is_err());
        unsafe { std::env::remove_var(BIND_ENV) };
    }

    #[test]
    fn pool_size_env_accepts_positive_integers() {
        assert_eq!(pool_size_from_env(Some("12")), Some(12));
        assert_eq!(pool_size_from_env(Some(" 100 ")), Some(100));
        assert_eq!(pool_size_from_env(Some("1")), Some(1));
    }

    #[test]
    fn pool_size_env_rejects_invalid_inputs() {
        assert_eq!(pool_size_from_env(None), None);
        assert_eq!(pool_size_from_env(Some("")), None);
        assert_eq!(pool_size_from_env(Some("   ")), None);
        assert_eq!(pool_size_from_env(Some("0")), None);
        assert_eq!(pool_size_from_env(Some("-3")), None);
        assert_eq!(pool_size_from_env(Some("abc")), None);
        assert_eq!(pool_size_from_env(Some("1.5")), None);
    }

    #[test]
    fn detected_pool_size_returns_at_least_one() {
        assert!(detected_pool_size() >= 1);
    }

    #[test]
    fn pool_size_from_perf_cores_reserves_headroom_above_threshold() {
        assert_eq!(pool_size_from_perf_cores(10), 8);
        assert_eq!(pool_size_from_perf_cores(8), 6);
        assert_eq!(pool_size_from_perf_cores(16), 14);
        assert_eq!(pool_size_from_perf_cores(6), 4);
    }

    #[test]
    fn pool_size_from_perf_cores_passes_small_topologies_through() {
        assert_eq!(pool_size_from_perf_cores(5), 5);
        assert_eq!(pool_size_from_perf_cores(4), 4);
        assert_eq!(pool_size_from_perf_cores(2), 2);
        assert_eq!(pool_size_from_perf_cores(1), 1);
    }

    #[test]
    fn blocking_cap_env_accepts_positive_integers() {
        assert_eq!(blocking_cap_from_env(Some("16")), Some(16));
        assert_eq!(blocking_cap_from_env(Some(" 64 ")), Some(64));
        assert_eq!(blocking_cap_from_env(Some("1")), Some(1));
    }

    #[test]
    fn blocking_cap_env_rejects_invalid_inputs() {
        assert_eq!(blocking_cap_from_env(None), None);
        assert_eq!(blocking_cap_from_env(Some("")), None);
        assert_eq!(blocking_cap_from_env(Some("   ")), None);
        assert_eq!(blocking_cap_from_env(Some("0")), None);
        assert_eq!(blocking_cap_from_env(Some("-3")), None);
        assert_eq!(blocking_cap_from_env(Some("abc")), None);
    }

    #[test]
    fn detected_blocking_cap_scales_with_cpus() {
        let cap = detected_blocking_cap();
        let cpus = detected_pool_size();
        assert!(cap >= cpus, "cap {cap} should be at least cpus {cpus}");
        assert!(cap >= 4, "cap {cap} should never fall below the 4-thread floor");
    }

    #[test]
    fn mb_from_env_accepts_positive_integers() {
        assert_eq!(mb_from_env(Some("128")), Some(128));
        assert_eq!(mb_from_env(Some(" 1024 ")), Some(1024));
        assert_eq!(mb_from_env(Some("1")), Some(1));
    }

    #[test]
    fn mb_from_env_rejects_invalid_inputs() {
        assert_eq!(mb_from_env(None), None);
        assert_eq!(mb_from_env(Some("")), None);
        assert_eq!(mb_from_env(Some("   ")), None);
        assert_eq!(mb_from_env(Some("0")), None);
        assert_eq!(mb_from_env(Some("-128")), None);
        assert_eq!(mb_from_env(Some("128.0")), None);
        assert_eq!(mb_from_env(Some("abc")), None);
    }

    #[test]
    fn pool_size_from_memory_budget_reserves_one_isolate_for_recycle_peak() {
        assert_eq!(pool_size_from_memory_budget(1024, 128), 7);
        assert_eq!(pool_size_from_memory_budget(640, 128), 4);
    }

    #[test]
    fn pool_size_from_memory_budget_floors_at_one_when_budget_too_small() {
        assert_eq!(pool_size_from_memory_budget(64, 128), 1);
        assert_eq!(pool_size_from_memory_budget(128, 128), 1);
        assert_eq!(pool_size_from_memory_budget(0, 128), 1);
    }

    #[test]
    fn pool_size_from_memory_budget_handles_zero_per_isolate() {
        let result = pool_size_from_memory_budget(1024, 0);
        assert!(result >= 1);
    }

    #[test]
    fn default_heap_per_isolate_mb_is_documented_empirical_value() {
        assert_eq!(DEFAULT_HEAP_PER_ISOLATE_MB, 64);
    }

    #[test]
    fn small_container_budget_unlocks_multiple_workers_after_p1() {
        assert_eq!(
            pool_size_from_memory_budget(256, DEFAULT_HEAP_PER_ISOLATE_MB),
            3
        );
        assert_eq!(
            pool_size_from_memory_budget(512, DEFAULT_HEAP_PER_ISOLATE_MB),
            7
        );
        assert_eq!(
            pool_size_from_memory_budget(1024, DEFAULT_HEAP_PER_ISOLATE_MB),
            15
        );
    }

    #[test]
    fn compose_default_v8_flags_omits_old_space_when_no_budget() {
        let flags = compose_default_v8_flags(None, 4);
        assert!(flags.contains("--max-semi-space-size=16"));
        assert!(!flags.contains("--max-old-space-size"));
    }

    #[test]
    fn compose_default_v8_flags_scales_old_space_with_budget_and_workers() {
        let flags = compose_default_v8_flags(Some(1024), 4);
        assert!(flags.contains("--max-old-space-size=192"));
        let flags = compose_default_v8_flags(Some(256), 1);
        assert!(flags.contains("--max-old-space-size=192"));
        let flags = compose_default_v8_flags(Some(1024), 2);
        assert!(flags.contains(&format!(
            "--max-old-space-size={HARD_OLD_SPACE_CAP_MB}"
        )));
    }

    #[test]
    fn compose_default_v8_flags_loosens_ceiling_with_large_budget() {
        let flags = compose_default_v8_flags(Some(1024), 1);
        assert!(flags.contains("--max-old-space-size=480"));
        let flags = compose_default_v8_flags(Some(8192), 1);
        assert!(flags.contains("--max-old-space-size=4064"));
    }

    #[test]
    fn compose_default_v8_flags_floors_at_min_cap() {
        let flags = compose_default_v8_flags(Some(96), 1);
        assert!(flags.contains(&format!("--max-old-space-size={MIN_OLD_SPACE_CAP_MB}")));
        let flags = compose_default_v8_flags(Some(512), 0);
        assert!(flags.contains(&format!(
            "--max-old-space-size={HARD_OLD_SPACE_CAP_MB}"
        )));
    }

    #[test]
    fn resolve_v8_flags_returns_composed_default_when_env_missing_or_blank() {
        let expected = compose_default_v8_flags(Some(1024), 4);
        assert_eq!(resolve_v8_flags(None, Some(1024), 4), expected);
        assert_eq!(resolve_v8_flags(Some(""), Some(1024), 4), expected);
        assert_eq!(resolve_v8_flags(Some("   "), Some(1024), 4), expected);
    }

    #[test]
    fn resolve_v8_flags_honours_env_override() {
        assert_eq!(
            resolve_v8_flags(Some("--max-old-space-size=128"), Some(1024), 4),
            "--max-old-space-size=128"
        );
        assert_eq!(
            resolve_v8_flags(Some("  --no-opt --max-old-space-size=32  "), None, 1),
            "--no-opt --max-old-space-size=32"
        );
    }

    #[test]
    fn resolve_max_inflight_falls_back_when_env_missing_or_blank() {
        assert_eq!(
            resolve_max_inflight_per_isolate(None),
            DEFAULT_MAX_INFLIGHT_PER_ISOLATE
        );
        assert_eq!(
            resolve_max_inflight_per_isolate(Some("")),
            DEFAULT_MAX_INFLIGHT_PER_ISOLATE
        );
        assert_eq!(
            resolve_max_inflight_per_isolate(Some("   ")),
            DEFAULT_MAX_INFLIGHT_PER_ISOLATE
        );
    }

    #[test]
    fn resolve_max_inflight_parses_unsigned_integers() {
        assert_eq!(resolve_max_inflight_per_isolate(Some("1")), 1);
        assert_eq!(resolve_max_inflight_per_isolate(Some("32")), 32);
        assert_eq!(resolve_max_inflight_per_isolate(Some("  64  ")), 64);
    }

    #[test]
    fn resolve_max_inflight_clamps_zero_up_to_one() {
        assert_eq!(resolve_max_inflight_per_isolate(Some("0")), 1);
    }

    #[test]
    fn resolve_max_inflight_falls_back_on_garbage() {
        assert_eq!(
            resolve_max_inflight_per_isolate(Some("not-a-number")),
            DEFAULT_MAX_INFLIGHT_PER_ISOLATE
        );
        assert_eq!(
            resolve_max_inflight_per_isolate(Some("-1")),
            DEFAULT_MAX_INFLIGHT_PER_ISOLATE
        );
    }

    #[test]
    fn default_max_inflight_matches_rubber_duck_recommendation() {
        assert_eq!(DEFAULT_MAX_INFLIGHT_PER_ISOLATE, 16);
    }

    #[test]
    fn runtime_mode_env_pins_single_thread() {
        for raw in ["single", "Single", "SINGLE-THREAD", "single_thread", "1", " single "] {
            let mode = resolve_runtime_mode(Some(raw), Some(64));
            assert_eq!(mode, RuntimeMode::SingleThread, "expected single for {raw:?}");
        }
    }

    #[test]
    fn runtime_mode_env_pins_multi_thread() {
        for raw in ["multi", "Multi-Thread", "multi_thread"] {
            let mode = resolve_runtime_mode(Some(raw), Some(1));
            assert_eq!(mode, RuntimeMode::MultiThread, "expected multi for {raw:?}");
        }
    }

    #[test]
    fn runtime_mode_auto_uses_cpu_count_for_one_cpu_hosts() {
        assert_eq!(
            resolve_runtime_mode(None, Some(1)),
            RuntimeMode::SingleThread
        );
        assert_eq!(
            resolve_runtime_mode(Some("auto"), Some(1)),
            RuntimeMode::SingleThread
        );
        assert_eq!(
            resolve_runtime_mode(Some(""), Some(1)),
            RuntimeMode::SingleThread
        );
    }

    #[test]
    fn runtime_mode_auto_picks_multi_thread_for_multi_cpu_hosts() {
        assert_eq!(resolve_runtime_mode(None, Some(2)), RuntimeMode::MultiThread);
        assert_eq!(resolve_runtime_mode(None, Some(8)), RuntimeMode::MultiThread);
    }

    #[test]
    fn runtime_mode_auto_falls_back_to_multi_when_cpu_unknown() {
        assert_eq!(resolve_runtime_mode(None, None), RuntimeMode::MultiThread);
    }

    #[test]
    fn runtime_mode_unknown_value_falls_back_to_auto() {
        assert_eq!(
            resolve_runtime_mode(Some("garbage"), Some(1)),
            RuntimeMode::SingleThread
        );
        assert_eq!(
            resolve_runtime_mode(Some("garbage"), Some(4)),
            RuntimeMode::MultiThread
        );
    }

    #[test]
    fn runtime_mode_env_constant_matches_documented_name() {
        assert_eq!(RUNTIME_MODE_ENV, "NEXIDE_RUNTIME_MODE");
    }

    #[test]
    fn runtime_mode_label_round_trips() {
        assert_eq!(RuntimeMode::SingleThread.label(), "single-thread");
        assert_eq!(RuntimeMode::MultiThread.label(), "multi-thread");
    }

    #[test]
    fn parse_bind_accepts_ipv4_host() {
        let addr = parse_bind("127.0.0.1", 4000).expect("ipv4");
        assert_eq!(addr.to_string(), "127.0.0.1:4000");
    }

    #[test]
    fn parse_bind_accepts_unspecified_host() {
        let addr = parse_bind("0.0.0.0", 8080).expect("any");
        assert_eq!(addr.to_string(), "0.0.0.0:8080");
    }

    #[test]
    fn parse_bind_wraps_ipv6_literals_in_brackets() {
        let addr = parse_bind("::1", 7000).expect("ipv6");
        assert_eq!(addr.to_string(), "[::1]:7000");
    }

    #[test]
    fn parse_bind_rejects_non_ip_hostnames() {
        let err = parse_bind("not-an-ip", 3000).expect_err("DNS names not supported");
        assert!(matches!(err, RuntimeError::InvalidBind { .. }));
    }

    #[test]
    fn absolute_dir_resolves_relative_to_cwd() {
        let tmp = tempfile::tempdir().expect("tmp");
        let resolved = absolute_dir(tmp.path()).expect("dir exists");
        assert!(resolved.is_absolute());
        assert!(resolved.is_dir());
    }

    #[test]
    fn absolute_dir_returns_missing_dir_when_absent() {
        let bogus = std::path::PathBuf::from("/definitely/does/not/exist/nexide");
        let err = absolute_dir(&bogus).expect_err("missing");
        assert!(matches!(err, RuntimeError::MissingDir(_)));
    }

    #[test]
    fn app_layout_reports_missing_directories() {
        let tmp = tempfile::tempdir().expect("tmp");
        let bind: std::net::SocketAddr = "127.0.0.1:3000".parse().unwrap();
        let err = AppLayout::resolve(tmp.path(), bind).expect_err("missing layout");
        assert!(matches!(err, RuntimeError::MissingDir(_)));
    }

    #[test]
    fn app_layout_resolves_standalone_dir_when_server_js_present() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path();
        std::fs::write(root.join("server.js"), "// noop\n").unwrap();
        std::fs::create_dir_all(root.join("public")).unwrap();
        std::fs::create_dir_all(root.join(".next/static")).unwrap();
        std::fs::create_dir_all(root.join(".next/server/app")).unwrap();
        let bind: std::net::SocketAddr = "127.0.0.1:3000".parse().unwrap();
        let layout = AppLayout::resolve(root, bind).expect("standalone layout");
        assert_eq!(layout.entrypoint.path, root.join("server.js"));
        assert_eq!(layout.entrypoint.kind.label(), "next_standalone");
    }

    #[test]
    fn app_layout_standalone_dir_missing_public_reports_path() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path();
        std::fs::write(root.join("server.js"), "// noop\n").unwrap();
        std::fs::create_dir_all(root.join(".next/static")).unwrap();
        std::fs::create_dir_all(root.join(".next/server/app")).unwrap();
        let bind: std::net::SocketAddr = "127.0.0.1:3000".parse().unwrap();
        let err = AppLayout::resolve(root, bind).expect_err("missing public");
        match err {
            RuntimeError::MissingDir(p) => {
                assert!(p.ends_with("public"), "unexpected path {}", p.display());
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }
}
