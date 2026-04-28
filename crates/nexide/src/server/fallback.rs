//! Abstraction over the dynamic layer (SSR / Route Handlers).
//!
//! This module deliberately knows nothing about V8 or Next.js — that is
//! the dependency-inversion boundary. The production implementation is
//! introduced earlier (bridge to `server.js`); here we provide a
//! minimal [`NotImplementedHandler`] that doubles as a safe fallback
//! and as a fixture in the static-layer tests.

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, Response};
use thiserror::Error;
#[cfg(test)]
use axum::http::StatusCode;

/// Errors raised by a [`DynamicHandler`].
#[derive(Debug, Error)]
pub enum HandlerError {
    /// The execution component rejected the request.
    #[error("upstream rejected the request: {0}")]
    Upstream(String),
}

/// Consumer of dynamic HTTP requests not handled by the static layer.
///
/// Every implementation must be safe to share between threads — Axum
/// clones the handle for every incoming request.
#[async_trait]
pub trait DynamicHandler: Send + Sync + 'static {
    /// Handles a single request.
    ///
    /// # Errors
    /// Returns [`HandlerError`] when the upstream rejects the request.
    /// The transport layer turns the error into an HTTP 502 response.
    async fn handle(&self, req: Request<Body>) -> Result<Response<Body>, HandlerError>;
}

/// Safe default handler returning `501 Not Implemented`.
///
/// Test-only: production code uses
/// [`super::next_bridge::NextBridgeHandler`] backed by an
/// [`crate::dispatch::IsolateDispatcher`]. Keeping this fixture
/// behind `cfg(test)` avoids dead code in the binary.
#[cfg(test)]
#[derive(Debug, Default, Clone, Copy)]
pub struct NotImplementedHandler;

#[cfg(test)]
#[async_trait]
impl DynamicHandler for NotImplementedHandler {
    async fn handle(&self, _req: Request<Body>) -> Result<Response<Body>, HandlerError> {
        let response = Response::builder()
            .status(StatusCode::NOT_IMPLEMENTED)
            .header("content-type", "text/plain; charset=utf-8")
            .body(Body::from("dynamic handler not yet implemented"))
            .expect("static builder cannot fail");
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::{DynamicHandler, HandlerError, NotImplementedHandler};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;

    #[test]
    fn handler_error_display_is_stable() {
        let err = HandlerError::Upstream("boom".into());
        assert!(err.to_string().starts_with("upstream rejected"));
    }

    #[tokio::test]
    async fn not_implemented_handler_returns_501() {
        let handler = NotImplementedHandler;
        let request = Request::builder()
            .uri("/dynamic")
            .body(Body::empty())
            .expect("request builder");
        let response = handler.handle(request).await.expect("infallible");
        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body collect")
            .to_bytes();
        assert_eq!(bytes.as_ref(), b"dynamic handler not yet implemented");
    }
}
