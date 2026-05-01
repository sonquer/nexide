//! HTTP shield (`Tarcza Rust`) for the `nexide` runtime.
//!
//! Combines the static layer (zero-copy `ServeDir`) with the dynamic
//! handler behind a single Axum frontend.

pub mod accept_loop;
pub mod config;
mod error_page;
pub mod fallback;
mod next_bridge;
mod prerender;
mod static_assets;
mod stream_listener;
pub mod worker_runtime;

use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::http::HeaderValue;
use axum::http::header::CACHE_CONTROL;
use thiserror::Error;
use tokio::net::TcpListener;
use tower::ServiceBuilder;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::TraceLayer;

pub use self::accept_loop::{AcceptError, run_accept_loop};
pub use self::config::{ConfigError, ServerConfig};
pub use self::fallback::{DynamicHandler, HandlerError};
pub use self::next_bridge::{MAX_REQUEST_BODY_BYTES, NextBridgeHandler};
pub use self::worker_runtime::{AcceptStrategy, WorkerRuntime, WorkerSpawnError};

#[cfg(test)]
pub use self::fallback::NotImplementedHandler;

/// Errors raised while running the HTTP shield.
#[derive(Debug, Error)]
pub enum ServerError {
    /// The listening socket could not be bound.
    #[error("failed to bind tcp listener: {0}")]
    Bind(#[source] std::io::Error),

    /// The `axum::serve` loop terminated with an error.
    #[error("server loop terminated with error: {0}")]
    Serve(#[source] std::io::Error),

    /// One of the per-worker runtimes failed to start.
    #[error("worker spawn failed: {0}")]
    Worker(#[from] WorkerSpawnError),

    /// The accept loop returned a fatal error.
    #[error("accept loop failed: {0}")]
    Accept(#[from] AcceptError),
}

/// Builds the Axum router that combines the static layer with the
/// injected dynamic handler.
///
/// Layering (highest priority first):
///   1. `/_next/static/*` - build chunks served zero-copy.
///   2. `/<route>` - `public/` static files (zero-copy).
///   3. `/<route>` - prerendered HTML/RSC out of `app_dir` (RAM cache,
///      mtime-validated). Bypasses V8 entirely for SSG/ISR pages.
///   4. `/<route>` - dynamic handler (V8 isolate pool) for everything
///      else (API routes, force-dynamic SSR, not-found, etc.).
///
/// The router is decoupled from any concrete [`DynamicHandler`]
/// implementation (DIP). Production code injects `NextBridgeHandler`
/// from the historical handler implementation; tests inject [`NotImplementedHandler`] or a recording
/// double.
pub fn build_router(cfg: &ServerConfig, handler: Arc<dyn DynamicHandler>) -> Router {
    let dynamic = static_assets::dynamic_service(handler.clone());
    let prerender = prerender::prerender_with_fallback(cfg.app_dir().to_path_buf(), dynamic);
    let public = static_assets::public_with_fallback_service(cfg.public_dir(), prerender);
    let next_image = crate::image::next_image_service_with_dynamic(
        cfg.app_dir().to_path_buf(),
        cfg.public_dir().to_path_buf(),
        cfg.next_static_dir().to_path_buf(),
        cfg.bind(),
        Some(handler),
    );
    let immutable_cache = ServiceBuilder::new().layer(SetResponseHeaderLayer::overriding(
        CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    ));
    Router::new()
        .nest_service(
            "/_next/static",
            immutable_cache.service(static_assets::next_static_only(cfg.next_static_dir())),
        )
        .nest_service("/_next/image", next_image)
        .fallback_service(public)
        .layer(TraceLayer::new_for_http())
}

/// Runs the HTTP shield until `shutdown` resolves.
///
/// Pure command (CQS): performs I/O and reports success or failure
/// without returning domain data.
///
/// # Errors
/// [`ServerError::Bind`] when the listening socket cannot be opened;
/// [`ServerError::Serve`] when `axum::serve` returns an error.
pub async fn serve<F>(
    cfg: &ServerConfig,
    handler: Arc<dyn DynamicHandler>,
    shutdown: F,
) -> Result<(), ServerError>
where
    F: Future<Output = ()> + Send + 'static,
{
    let listener = TcpListener::bind(cfg.bind())
        .await
        .map_err(ServerError::Bind)?;
    let local = listener.local_addr().map_err(ServerError::Bind)?;
    tracing::info!(%local, "nexide shield listening");
    let app = build_router(cfg, handler);
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
        .map_err(ServerError::Serve)
}

/// Environment variable that disables the Linux `SO_REUSEPORT`
/// per-worker fast path.
///
/// Recognised values are `0`, `false`, `off`, `no` (case-insensitive).
/// Any other value - or unset - keeps the fast path enabled on Linux.
///
/// On non-Linux platforms (`target_os != "linux"`) the env var is
/// ignored: the shared-listener path is the only supported model
/// because BSD-derived kernels expose `SO_REUSEPORT` semantics
/// (last-binder-wins) that are not load-balancing.
pub const REUSE_PORT_ENV: &str = "NEXIDE_REUSE_PORT";

/// Returns `true` when the operator-supplied env value disables the
/// `SO_REUSEPORT` fast path.
///
/// Pure helper so the resolution rule is unit-testable without
/// touching the process environment.
#[must_use]
pub fn reuseport_disabled_by_env(raw: Option<&str>) -> bool {
    matches!(
        raw.map(str::trim).map(str::to_ascii_lowercase).as_deref(),
        Some("0" | "false" | "off" | "no")
    )
}

/// Runs the HTTP shield with `worker_count` per-worker runtimes.
///
/// Each worker hosts its own `current_thread` Tokio runtime, its own
/// V8 isolate, and its own copy of the Axum router. Two acceptance
/// strategies are supported:
///
/// 1. **Linux fast path** - every worker binds its own
///    `TcpListener` with `SO_REUSEPORT`; the kernel distributes
///    incoming connections via 4-tuple hash. There is no central
///    accept loop, no mpsc hop, and the main reactor only awaits
///    `shutdown` and broadcasts it to every worker.
/// 2. **Shared-listener fallback** - the main reactor
///    binds a single [`TcpListener`] and runs [`run_accept_loop`]
///    which routes incoming connections to per-worker mpsc
///    mailboxes via adaptive *power of two choices* over their
///    queue depth. Used on macOS / Windows, when the `SO_REUSEPORT`
///    bind fails on Linux (logged at `warn!`), and when the
///    operator opts out via [`REUSE_PORT_ENV`].
///
/// `worker_count` must be ≥ 1; values larger than the number of
/// available CPU cores are accepted but typically yield diminishing
/// returns.
///
/// Pure command (CQS): performs I/O and reports success or failure
/// without returning domain data. The returned future resolves once
/// every worker has drained its in-flight connections.
///
/// # Errors
///
/// [`ServerError::Bind`] when the listening socket cannot be opened;
/// [`ServerError::Worker`] when any worker fails to boot;
/// [`ServerError::Accept`] when the accept loop reports a fatal
/// error.
pub async fn serve_with_workers<F>(
    cfg: ServerConfig,
    entrypoint: PathBuf,
    worker_count: usize,
    shutdown: F,
) -> Result<(), ServerError>
where
    F: Future<Output = ()> + Send + 'static,
{
    let worker_count = worker_count.max(1);
    let reuseport_requested = is_reuseport_requested();
    if !reuseport_requested {
        return serve_with_shared_listener(cfg, entrypoint, worker_count, shutdown).await;
    }
    match try_serve_reuseport(cfg.clone(), entrypoint.clone(), worker_count, shutdown).await {
        Ok(outcome) => outcome,
        Err(ReusePortFallback { reason, shutdown }) => {
            tracing::warn!(
                %reason,
                "SO_REUSEPORT fast path unavailable - falling back to shared listener"
            );
            serve_with_shared_listener(cfg, entrypoint, worker_count, shutdown).await
        }
    }
}

#[cfg(target_os = "linux")]
fn is_reuseport_requested() -> bool {
    !reuseport_disabled_by_env(std::env::var(REUSE_PORT_ENV).ok().as_deref())
}

#[cfg(not(target_os = "linux"))]
const fn is_reuseport_requested() -> bool {
    false
}

/// Reason the `SO_REUSEPORT` fast path declined to take ownership of
/// the connection lifecycle, returned with the still-pending shutdown
/// future so the caller can fall back to the shared-listener path
/// without re-arming a fresh signal handler.
struct ReusePortFallback<F> {
    reason: String,
    shutdown: F,
}

#[cfg(target_os = "linux")]
async fn try_serve_reuseport<F>(
    cfg: ServerConfig,
    entrypoint: PathBuf,
    worker_count: usize,
    shutdown: F,
) -> Result<Result<(), ServerError>, ReusePortFallback<F>>
where
    F: Future<Output = ()> + Send + 'static,
{
    let local = cfg.bind();
    let mut joinset = tokio::task::JoinSet::<Result<WorkerRuntime, WorkerSpawnError>>::new();
    for idx in 0..worker_count {
        let cfg = cfg.clone();
        let entry = entrypoint.clone();
        joinset.spawn(async move {
            WorkerRuntime::spawn_reuseport(idx, worker_count, cfg, entry).await
        });
    }

    let mut workers: Vec<WorkerRuntime> = Vec::with_capacity(worker_count);
    while let Some(joined) = joinset.join_next().await {
        match joined {
            Ok(Ok(worker)) => workers.push(worker),
            Ok(Err(WorkerSpawnError::ReusePortBind { addr, source })) => {
                joinset.shutdown().await;
                shutdown_workers(workers);
                return Err(ReusePortFallback {
                    reason: format!("reuseport bind failed on {addr}: {source}"),
                    shutdown,
                });
            }
            Ok(Err(err)) => {
                joinset.shutdown().await;
                shutdown_workers(workers);
                return Ok(Err(ServerError::Worker(err)));
            }
            Err(panic) => {
                joinset.shutdown().await;
                shutdown_workers(workers);
                return Ok(Err(ServerError::Worker(WorkerSpawnError::Engine(format!(
                    "worker boot task panicked: {panic}"
                )))));
            }
        }
    }
    workers.sort_by_key(WorkerRuntime::idx);

    tracing::info!(
        %local,
        workers = worker_count,
        strategy = "reuseport",
        "nexide shield listening"
    );

    shutdown.await;
    tracing::info!("nexide shield: shutdown signal received");
    shutdown_workers(workers);
    Ok(Ok(()))
}

/// Two-phase shutdown of a worker fleet: signal **all** workers to
/// stop accepting before joining any of them. Without this split a
/// single slow drain would let later workers continue serving fresh
/// traffic until earlier workers fully exit.
#[cfg(target_os = "linux")]
fn shutdown_workers(workers: Vec<WorkerRuntime>) {
    for w in &workers {
        w.signal_shutdown();
    }
    for w in workers {
        w.join();
    }
}

#[cfg(not(target_os = "linux"))]
#[allow(
    clippy::unused_async,
    reason = "matches the Linux signature; the caller awaits the returned future on both targets"
)]
async fn try_serve_reuseport<F>(
    _cfg: ServerConfig,
    _entrypoint: PathBuf,
    _worker_count: usize,
    shutdown: F,
) -> Result<Result<(), ServerError>, ReusePortFallback<F>>
where
    F: Future<Output = ()> + Send + 'static,
{
    Err(ReusePortFallback {
        reason: "SO_REUSEPORT fast path is Linux-only".to_owned(),
        shutdown,
    })
}

async fn serve_with_shared_listener<F>(
    cfg: ServerConfig,
    entrypoint: PathBuf,
    worker_count: usize,
    shutdown: F,
) -> Result<(), ServerError>
where
    F: Future<Output = ()> + Send + 'static,
{
    let listener = TcpListener::bind(cfg.bind())
        .await
        .map_err(ServerError::Bind)?;
    let local = listener.local_addr().map_err(ServerError::Bind)?;
    tracing::info!(
        %local,
        workers = worker_count,
        strategy = "shared",
        "nexide shield listening"
    );

    let mut joinset = tokio::task::JoinSet::<Result<WorkerRuntime, WorkerSpawnError>>::new();
    for idx in 0..worker_count {
        let cfg = cfg.clone();
        let entry = entrypoint.clone();
        joinset.spawn(async move { WorkerRuntime::spawn(idx, worker_count, cfg, entry).await });
    }
    let mut workers: Vec<WorkerRuntime> = Vec::with_capacity(worker_count);
    while let Some(joined) = joinset.join_next().await {
        match joined {
            Ok(Ok(worker)) => workers.push(worker),
            Ok(Err(err)) => {
                joinset.shutdown().await;
                for w in workers {
                    w.shutdown();
                }
                return Err(ServerError::Worker(err));
            }
            Err(panic) => {
                joinset.shutdown().await;
                for w in workers {
                    w.shutdown();
                }
                return Err(ServerError::Worker(WorkerSpawnError::Engine(format!(
                    "worker boot task panicked: {panic}"
                ))));
            }
        }
    }
    workers.sort_by_key(WorkerRuntime::idx);

    let shared: Arc<[WorkerRuntime]> = Arc::from(workers);
    let outcome = run_accept_loop(listener, Arc::clone(&shared), shutdown).await;
    drop(shared);
    outcome.map_err(ServerError::Accept)
}

#[cfg(test)]
mod tests {
    use super::{
        DynamicHandler, NotImplementedHandler, ServerConfig, ServerError, build_router, serve,
    };
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tower::ServiceExt;

    fn fixture() -> (TempDir, TempDir, TempDir, ServerConfig) {
        let pub_dir = TempDir::new().expect("tempdir");
        let static_dir = TempDir::new().expect("tempdir");
        let app_dir = TempDir::new().expect("tempdir");
        std::fs::write(pub_dir.path().join("hello.txt"), b"world").expect("write");
        std::fs::write(static_dir.path().join("chunk.js"), b"// js").expect("write");
        let bind: SocketAddr = "127.0.0.1:0".parse().expect("addr");
        let cfg = ServerConfig::try_new(
            bind,
            pub_dir.path().to_path_buf(),
            static_dir.path().to_path_buf(),
            app_dir.path().to_path_buf(),
        )
        .expect("valid config");
        (pub_dir, static_dir, app_dir, cfg)
    }

    #[tokio::test]
    async fn router_serves_public_files() {
        let (_p, _s, _a, cfg) = fixture();
        let handler: Arc<dyn DynamicHandler> = Arc::new(NotImplementedHandler);
        let router = build_router(&cfg, handler);
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/hello.txt")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("infallible");
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        assert_eq!(bytes.as_ref(), b"world");
    }

    #[tokio::test]
    async fn router_serves_next_static_chunks() {
        let (_p, _s, _a, cfg) = fixture();
        let router = build_router(&cfg, Arc::new(NotImplementedHandler));
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/_next/static/chunk.js")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("infallible");
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn router_serves_prerendered_html_without_dynamic_handler() {
        let (_p, _s, app_dir, cfg) = fixture();
        std::fs::write(app_dir.path().join("about.html"), b"<html>about</html>")
            .expect("write prerender");
        let router = build_router(&cfg, Arc::new(NotImplementedHandler));
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/about")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("infallible");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get("x-nextjs-cache")
                .map(|h| h.to_str().unwrap()),
            Some("HIT"),
        );
    }

    #[tokio::test]
    async fn router_falls_back_to_dynamic_for_unknown_routes() {
        let (_p, _s, _a, cfg) = fixture();
        let router = build_router(&cfg, Arc::new(NotImplementedHandler));
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/dynamic/route")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("infallible");
        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    }

    #[tokio::test]
    async fn router_blocks_path_traversal() {
        let (_p, _s, _a, cfg) = fixture();
        let router = build_router(&cfg, Arc::new(NotImplementedHandler));
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/..%2F..%2Fetc%2Fpasswd")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("infallible");
        assert_ne!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn serve_errors_when_bind_fails() {
        let (_p, _s, _a, cfg) = fixture();
        let already = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ok");
        let occupied = already.local_addr().expect("addr");
        let occupied_cfg = ServerConfig::try_new(
            occupied,
            cfg.public_dir().to_path_buf(),
            cfg.next_static_dir().to_path_buf(),
            cfg.app_dir().to_path_buf(),
        )
        .expect("config");
        let handler: Arc<dyn DynamicHandler> = Arc::new(NotImplementedHandler);
        let result = serve(&occupied_cfg, handler, async {}).await;
        assert!(matches!(result, Err(ServerError::Bind(_))));
        drop(already);
    }

    #[test]
    fn reuseport_disabled_by_env_recognises_falsy_values() {
        for raw in ["0", "false", "FALSE", " off ", "no", "No"] {
            assert!(
                super::reuseport_disabled_by_env(Some(raw)),
                "expected {raw:?} to disable reuseport"
            );
        }
    }

    #[test]
    fn reuseport_disabled_by_env_keeps_default_for_other_values() {
        for raw in [
            None,
            Some(""),
            Some("1"),
            Some("true"),
            Some("yes"),
            Some("on"),
        ] {
            assert!(
                !super::reuseport_disabled_by_env(raw),
                "expected {raw:?} to keep reuseport enabled"
            );
        }
    }
}
