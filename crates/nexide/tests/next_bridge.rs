//! Integration test for the Next.js bridge handler.
//!
//! Boots an [`IsolateDispatcher`] over a synthetic CJS handler that
//! mimics a Next.js standalone server (`http.createServer(...).listen(0)`)
//! and exercises the full HTTP shield via `tower::ServiceExt::oneshot`.
//! Validates Axum → dispatch → V8 handler → response on the same code
//! paths the production binary uses, without requiring a built bundle.

#![allow(clippy::future_not_send, clippy::significant_drop_tightening)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use nexide::dispatch::{EngineDispatcher, IsolateDispatcher};
use nexide::server::{NextBridgeHandler, ServerConfig, build_router};
use tempfile::TempDir;
use tower::ServiceExt;

const SYNTHETIC_HANDLER: &str = r#"
const http = require('node:http');

const handler = (req, res) => {
  if (req.url === "/api/ping") {
    res.writeHead(200, [["content-type", "application/json"]]);
    res.end(JSON.stringify({ ok: true, runtime: "nexide", method: req.method }));
    return;
  }
  const userAgent = (req.headers["user-agent"]) || "unknown";
  res.writeHead(200, [["content-type", "text/html; charset=utf-8"]]);
  res.end(
    "<!doctype html><main data-testid=\"ssr-marker\"><p>" +
      userAgent +
      "</p></main>"
  );
};

http.createServer(handler).listen(0);
"#;

/// Writes the synthetic CJS handler into a fresh tempdir and returns
/// its absolute path. The directory guard is held by the caller for
/// the lifetime of the test so the file is cleaned up afterwards.
fn synthetic_entrypoint() -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("handler.cjs");
    std::fs::write(&path, SYNTHETIC_HANDLER).expect("write handler");
    (dir, path)
}

/// Builds a [`ServerConfig`] with throwaway public/static/app
/// directories so the static-asset routes participate in the test
/// router without polluting the workspace.
fn fixture_config() -> (TempDir, TempDir, TempDir, ServerConfig) {
    let pub_dir = TempDir::new().expect("tempdir");
    let static_dir = TempDir::new().expect("tempdir");
    let app_dir = TempDir::new().expect("tempdir");
    std::fs::write(static_dir.path().join("chunk.js"), b"// js").expect("write chunk");
    std::fs::write(pub_dir.path().join("favicon.ico"), b"icon").expect("write favicon");
    let bind: SocketAddr = "127.0.0.1:0".parse().expect("bind");
    let cfg = ServerConfig::try_new(
        bind,
        pub_dir.path().to_path_buf(),
        static_dir.path().to_path_buf(),
        app_dir.path().to_path_buf(),
    )
    .expect("server cfg");
    (pub_dir, static_dir, app_dir, cfg)
}

#[tokio::test]
async fn ssr_route_returns_html_with_marker() {
    let (_handler_dir, entrypoint) = synthetic_entrypoint();
    let dispatcher = IsolateDispatcher::spawn(entrypoint)
        .await
        .expect("dispatcher");
    let dispatcher = Arc::new(dispatcher);
    let handler = Arc::new(NextBridgeHandler::new(dispatcher.clone()));
    let (_p, _s, _a, cfg) = fixture_config();
    let router = build_router(&cfg, handler);

    let response = router
        .oneshot(
            Request::builder()
                .uri("/")
                .header("user-agent", "nexide-test")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("infallible");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let text = std::str::from_utf8(&body).expect("utf8");
    assert!(
        text.contains("data-testid=\"ssr-marker\""),
        "missing SSR marker: {text}",
    );
    assert!(
        text.contains("nexide-test"),
        "user-agent header not propagated: {text}",
    );
    assert_eq!(dispatcher.dispatch_count(), 1);
}

#[tokio::test]
async fn api_route_returns_json() {
    let (_handler_dir, entrypoint) = synthetic_entrypoint();
    let dispatcher = IsolateDispatcher::spawn(entrypoint)
        .await
        .expect("dispatcher");
    let dispatcher = Arc::new(dispatcher);
    let handler = Arc::new(NextBridgeHandler::new(dispatcher.clone()));
    let (_p, _s, _a, cfg) = fixture_config();
    let router = build_router(&cfg, handler);

    let response = router
        .oneshot(
            Request::builder()
                .uri("/api/ping")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("infallible");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("application/json"),
    );
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let text = std::str::from_utf8(&body).expect("utf8");
    assert!(text.contains("\"ok\":true"), "unexpected json: {text}");
    assert!(text.contains("\"runtime\":\"nexide\""));
}

#[tokio::test]
async fn static_assets_bypass_isolate() {
    let (_handler_dir, entrypoint) = synthetic_entrypoint();
    let dispatcher = IsolateDispatcher::spawn(entrypoint)
        .await
        .expect("dispatcher");
    let dispatcher = Arc::new(dispatcher);
    let handler = Arc::new(NextBridgeHandler::new(dispatcher.clone()));
    let (_p, _s, _a, cfg) = fixture_config();
    let router = build_router(&cfg, handler);

    let before = dispatcher.dispatch_count();

    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/_next/static/chunk.js")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("infallible");
    assert_eq!(response.status(), StatusCode::OK);

    let response = router
        .oneshot(
            Request::builder()
                .uri("/favicon.ico")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("infallible");
    assert_eq!(response.status(), StatusCode::OK);

    assert_eq!(
        dispatcher.dispatch_count(),
        before,
        "static assets must not reach the isolate",
    );
}
