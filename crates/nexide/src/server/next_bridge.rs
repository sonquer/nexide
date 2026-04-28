//! Production [`DynamicHandler`] that forwards requests to a
//! JavaScript handler running inside [`crate::engine::V8Engine`].
//!
//! This is the runtime replacement for the test-only
//! [`super::fallback::NotImplementedHandler`]; it owns nothing more
//! than an [`crate::dispatch::EngineDispatcher`] handle and is fully
//! decoupled from the concrete engine implementation
//! (Dependency Inversion Principle).

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::header::{HeaderName, HeaderValue};
use axum::http::{HeaderMap, Request, Response, StatusCode};
use http_body_util::BodyExt;

use super::fallback::{DynamicHandler, HandlerError};
use crate::dispatch::{DispatchError, EngineDispatcher, ProtoRequest};
use crate::ops::HeaderPair;

/// Maximum buffered request body size. Larger bodies are rejected
/// with `413 Payload Too Large` to keep the worker thread bounded.
pub const MAX_REQUEST_BODY_BYTES: usize = 8 * 1024 * 1024;

/// HTTP shield handler that bridges Axum to the JavaScript handler.
///
/// Generic over the dispatcher to keep the production wiring testable;
/// the `next_bridge.rs` integration test substitutes an in-memory
/// [`EngineDispatcher`] double.
pub struct NextBridgeHandler<D> {
    dispatcher: Arc<D>,
}

impl<D> NextBridgeHandler<D>
where
    D: EngineDispatcher,
{
    /// Wraps `dispatcher` in an Axum-compatible handler.
    #[must_use]
    pub const fn new(dispatcher: Arc<D>) -> Self {
        Self { dispatcher }
    }

    /// Returns the underlying dispatcher (Query - used by tests for
    /// telemetry assertions).
    #[must_use]
    pub const fn dispatcher(&self) -> &Arc<D> {
        &self.dispatcher
    }
}

#[async_trait]
impl<D> DynamicHandler for NextBridgeHandler<D>
where
    D: EngineDispatcher,
{
    async fn handle(&self, req: Request<Body>) -> Result<Response<Body>, HandlerError> {
        let t_accept_start = Instant::now();
        let proto = match build_proto_request(req).await {
            Ok(p) => p,
            Err(err) => return Ok(error_response(&err)),
        };
        let accept_elapsed = t_accept_start.elapsed();

        let t_dispatch_start = Instant::now();
        let outcome = self.dispatcher.dispatch(proto).await;
        let dispatch_elapsed = t_dispatch_start.elapsed();

        let t_respond_start = Instant::now();
        let mut response = match outcome {
            Ok(payload) => payload_to_response(payload),
            Err(err) => error_response(&err),
        };
        let respond_elapsed = t_respond_start.elapsed();

        stamp_phase_breakdown(
            response.headers_mut(),
            accept_elapsed,
            dispatch_elapsed,
            respond_elapsed,
        );
        Ok(response)
    }
}

/// Appends per-phase Server-Timing metrics (`accept`, `dispatch_inner`,
/// `respond`) so a downstream stamp from [`crate::server::prerender`]
/// can prepend the canonical `srv;desc="v8-dispatch";dur=...` total
/// while keeping the breakdown visible to dev-tools and `curl -v`.
fn stamp_phase_breakdown(
    headers: &mut HeaderMap,
    accept: std::time::Duration,
    dispatch: std::time::Duration,
    respond: std::time::Duration,
) {
    let value = format!(
        "accept;dur={:.3}, dispatch_inner;dur={:.3}, respond;dur={:.3}",
        duration_ms(accept),
        duration_ms(dispatch),
        duration_ms(respond),
    );
    if let Ok(v) = HeaderValue::from_str(&value) {
        headers.append("server-timing", v);
    }
}

fn duration_ms(d: std::time::Duration) -> f64 {
    let micros = u64::try_from(d.as_micros()).unwrap_or(u64::MAX);
    #[allow(clippy::cast_precision_loss)]
    let ms = micros as f64 / 1000.0;
    ms
}

async fn build_proto_request(req: Request<Body>) -> Result<ProtoRequest, DispatchError> {
    let (parts, body) = req.into_parts();
    let method = parts.method.to_string();
    let uri = parts
        .uri
        .path_and_query()
        .map_or_else(|| parts.uri.to_string(), ToString::to_string);

    let mut headers = Vec::with_capacity(parts.headers.len());
    for (name, value) in &parts.headers {
        let value_str = match value.to_str() {
            Ok(v) => v.to_owned(),
            Err(_) => continue,
        };
        headers.push(HeaderPair {
            name: name.as_str().to_ascii_lowercase(),
            value: value_str,
        });
    }

    let collected = body
        .collect()
        .await
        .map_err(|err| DispatchError::BodyRead(err.to_string()))?;
    let bytes = collected.to_bytes();

    if bytes.len() > MAX_REQUEST_BODY_BYTES {
        return Err(DispatchError::BodyRead(format!(
            "request body exceeds {MAX_REQUEST_BODY_BYTES} bytes"
        )));
    }

    Ok(ProtoRequest {
        method,
        uri,
        headers,
        body: bytes,
    })
}

fn payload_to_response(payload: crate::ops::ResponsePayload) -> Response<Body> {
    let status = StatusCode::from_u16(payload.head.status).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut builder = Response::builder().status(status);
    let headers_mut = builder
        .headers_mut()
        .expect("response builder must accept headers");
    for (name, value) in payload.head.headers {
        let Ok(header_name) = HeaderName::try_from(name) else {
            continue;
        };
        let Ok(header_value) = HeaderValue::try_from(value) else {
            continue;
        };
        headers_mut.append(header_name, header_value);
    }
    builder
        .body(Body::from(payload.body))
        .unwrap_or_else(|_| infallible_502())
}

fn error_response(err: &DispatchError) -> Response<Body> {
    tracing::error!(error = %err, "next bridge dispatch failed");
    let (status, message) = match err {
        DispatchError::BadRequest(_) => (StatusCode::BAD_REQUEST, err.to_string()),
        DispatchError::WorkerGone | DispatchError::NoResponse => (
            StatusCode::SERVICE_UNAVAILABLE,
            "engine worker unavailable".to_owned(),
        ),
        _ => (StatusCode::BAD_GATEWAY, err.to_string()),
    };
    Response::builder()
        .status(status)
        .header("content-type", "text/plain; charset=utf-8")
        .body(Body::from(message))
        .unwrap_or_else(|_| infallible_502())
}

fn infallible_502() -> Response<Body> {
    let mut response = Response::new(Body::from("internal error"));
    *response.status_mut() = StatusCode::BAD_GATEWAY;
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::{ResponseHead, ResponsePayload};
    use bytes::Bytes;
    use http_body_util::BodyExt;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct EchoDispatcher {
        count: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl EngineDispatcher for EchoDispatcher {
        async fn dispatch(&self, request: ProtoRequest) -> Result<ResponsePayload, DispatchError> {
            self.count.fetch_add(1, Ordering::Relaxed);
            let body = Bytes::from(format!(
                "{}:{}",
                request.method,
                std::str::from_utf8(&request.body).unwrap_or("")
            ));
            Ok(ResponsePayload {
                head: ResponseHead {
                    status: 200,
                    headers: vec![("content-type".into(), "text/plain".into())],
                },
                body,
            })
        }

        fn dispatch_count(&self) -> usize {
            self.count.load(Ordering::Relaxed)
        }
    }

    #[tokio::test]
    async fn forwards_request_and_returns_response() {
        let dispatcher = Arc::new(EchoDispatcher {
            count: Arc::new(AtomicUsize::new(0)),
        });
        let handler = NextBridgeHandler::new(dispatcher.clone());
        let req = Request::builder()
            .method("POST")
            .uri("/echo")
            .body(Body::from("ping"))
            .expect("request");
        let response = handler.handle(req).await.expect("infallible");
        assert_eq!(response.status(), StatusCode::OK);
        let body = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        assert_eq!(&body[..], b"POST:ping");
        assert_eq!(dispatcher.dispatch_count(), 1);
    }

    #[tokio::test]
    async fn surfaces_worker_gone_as_503() {
        struct DeadDispatcher;
        #[async_trait]
        impl EngineDispatcher for DeadDispatcher {
            async fn dispatch(
                &self,
                _request: ProtoRequest,
            ) -> Result<ResponsePayload, DispatchError> {
                Err(DispatchError::WorkerGone)
            }
            fn dispatch_count(&self) -> usize {
                0
            }
        }

        let handler = NextBridgeHandler::new(Arc::new(DeadDispatcher));
        let req = Request::builder()
            .uri("/")
            .body(Body::empty())
            .expect("request");
        let response = handler.handle(req).await.expect("infallible");
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn rejects_oversized_body_with_502() {
        struct UnusedDispatcher;
        #[async_trait]
        impl EngineDispatcher for UnusedDispatcher {
            async fn dispatch(
                &self,
                _request: ProtoRequest,
            ) -> Result<ResponsePayload, DispatchError> {
                panic!("dispatcher must not be invoked");
            }
            fn dispatch_count(&self) -> usize {
                0
            }
        }

        let handler = NextBridgeHandler::new(Arc::new(UnusedDispatcher));
        let body = vec![b'x'; MAX_REQUEST_BODY_BYTES + 1];
        let req = Request::builder()
            .method("POST")
            .uri("/")
            .body(Body::from(body))
            .expect("request");
        let response = handler.handle(req).await.expect("infallible");
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    }
}
