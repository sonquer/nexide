//! Production [`DynamicHandler`] that forwards requests to a
//! JavaScript handler running inside [`crate::engine::V8Engine`].
//!
//! This is the runtime replacement for the test-only
//! [`super::fallback::NotImplementedHandler`]; it owns nothing more
//! than an [`crate::dispatch::EngineDispatcher`] handle and is fully
//! decoupled from the concrete engine implementation
//! (Dependency Inversion Principle).

use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Instant;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::header::{ACCEPT, HeaderName, HeaderValue};
use axum::http::{HeaderMap, Request, Response, StatusCode};
use http_body_util::BodyExt;
use tokio::sync::Semaphore;

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
///
/// Concurrency: when constructed via [`Self::with_inflight_limit`],
/// the handler gates the JS dispatch path behind a [`Semaphore`].
/// This is the *only* memory-bounded backpressure between the kernel
/// accept queue and the per-isolate JS heap; without it every
/// concurrent connection materialises a `DispatchJob` plus a Next.js
/// render context (~2-3 MiB live heap each), and a tight container
/// (e.g. `1 CPU / 256 MiB`) hits `Fatal JavaScript out of memory:
/// Ineffective mark-compacts near heap limit` long before V8's
/// `--max-old-space-size` cap is reached, because the GC death-spiral
/// triggers when reclamation cannot catch up with allocation. With a
/// permit cap of `N`, the steady-state working set is bounded at
/// roughly `N * per_render_mb`, regardless of how many TCP
/// connections hyper accepts.
pub struct NextBridgeHandler<D> {
    dispatcher: Arc<D>,
    inflight_limit: Option<Arc<Semaphore>>,
}

impl<D> NextBridgeHandler<D>
where
    D: EngineDispatcher,
{
    /// Wraps `dispatcher` in an Axum-compatible handler with no
    /// inflight cap.
    ///
    /// Prefer [`Self::with_inflight_limit`] in production to keep the
    /// JS heap bounded under bursty traffic; this constructor exists
    /// for unit tests where the dispatcher itself is a synchronous
    /// double and concurrency is bounded by the test harness.
    #[must_use]
    pub const fn new(dispatcher: Arc<D>) -> Self {
        Self {
            dispatcher,
            inflight_limit: None,
        }
    }

    /// Wraps `dispatcher` and gates JS dispatch behind a
    /// [`Semaphore`] capped at `permits`.
    ///
    /// `permits == 0` is silently clamped to `1` so the runtime
    /// always remains live (operators sometimes mis-key the env
    /// var). Pass `None` for unlimited concurrency.
    #[must_use]
    pub fn with_inflight_limit(dispatcher: Arc<D>, permits: Option<usize>) -> Self {
        let inflight_limit = permits.map(|n| Arc::new(Semaphore::new(n.max(1))));
        Self {
            dispatcher,
            inflight_limit,
        }
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
        let accept_header = req.headers().get(ACCEPT).cloned();
        let proto = match build_proto_request(req).await {
            Ok(p) => p,
            Err(err) => return Ok(error_response(&err, accept_header.as_ref())),
        };

        let accept_elapsed = t_accept_start.elapsed();

        let _permit = match &self.inflight_limit {
            Some(sem) => Some(
                Arc::clone(sem)
                    .acquire_owned()
                    .await
                    .expect("semaphore live"),
            ),
            None => None,
        };

        let t_dispatch_start = Instant::now();
        let outcome = self.dispatcher.dispatch(proto).await;
        let dispatch_elapsed = t_dispatch_start.elapsed();

        let t_respond_start = Instant::now();
        let mut response = match outcome {
            Ok(payload) => payload_to_response(payload),
            Err(err) => error_response(&err, accept_header.as_ref()),
        };
        let respond_elapsed = t_respond_start.elapsed();

        if phase_breakdown_enabled() {
            stamp_phase_breakdown(
                response.headers_mut(),
                accept_elapsed,
                dispatch_elapsed,
                respond_elapsed,
            );
        }
        Ok(response)
    }
}

/// Returns `true` when `NEXIDE_PHASE_BREAKDOWN=1` is set.
///
/// Resolved exactly once per process via [`OnceLock`] so the hot path
/// reads a cached `bool` instead of touching the env on every request.
/// The breakdown header is a developer aid (it stamps a multi-segment
/// `Server-Timing` value with `accept`/`dispatch_inner`/`respond`
/// durations) and adds a `format!()` + `HeaderValue::from_str()` plus a
/// header-table append per response, which shows up at high RPS. We
/// keep it disabled by default so production traffic pays nothing for
/// it; observability stacks that need the data set the env flag.
fn phase_breakdown_enabled() -> bool {
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| {
        matches!(
            std::env::var("NEXIDE_PHASE_BREAKDOWN").as_deref(),
            Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes")
        )
    })
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
        // `HeaderName::as_str()` already yields the canonical
        // lowercased form; the previous `to_ascii_lowercase()` call
        // forced an extra allocation on every header on every request
        // for a no-op transformation.
        headers.push(HeaderPair {
            name: name.as_str().to_owned(),
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

fn error_response(err: &DispatchError, accept: Option<&HeaderValue>) -> Response<Body> {
    tracing::error!(error = %err, "next bridge dispatch failed");
    let status = match err {
        DispatchError::BadRequest(_) => StatusCode::BAD_REQUEST,
        DispatchError::WorkerGone | DispatchError::NoResponse => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::BAD_GATEWAY,
    };
    let detail = err.to_string();
    super::error_page::render(status, accept, Some(&detail))
}

fn infallible_502() -> Response<Body> {
    super::error_page::render(StatusCode::BAD_GATEWAY, None, None)
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

    #[tokio::test]
    async fn inflight_semaphore_caps_concurrent_dispatches() {
        use std::sync::atomic::AtomicUsize;
        use std::time::Duration;

        // Dispatcher that parks every call on a shared release
        // counter, lets us observe how many handler tasks are in
        // `dispatch()` simultaneously. With permits=2, never more
        // than 2 should be parked at once even when we fire 8
        // concurrent requests.
        struct ParkDispatcher {
            parked: Arc<AtomicUsize>,
            peak: Arc<AtomicUsize>,
            release: Arc<tokio::sync::Semaphore>,
        }
        #[async_trait]
        impl EngineDispatcher for ParkDispatcher {
            async fn dispatch(
                &self,
                _request: ProtoRequest,
            ) -> Result<ResponsePayload, DispatchError> {
                let now = self.parked.fetch_add(1, Ordering::SeqCst) + 1;
                let prev = self.peak.load(Ordering::SeqCst);
                if now > prev {
                    self.peak.store(now, Ordering::SeqCst);
                }
                let _ = self.release.acquire().await.expect("live").forget();
                self.parked.fetch_sub(1, Ordering::SeqCst);
                Ok(ResponsePayload {
                    head: ResponseHead {
                        status: 200,
                        headers: vec![],
                    },
                    body: Vec::new().into(),
                })
            }
            fn dispatch_count(&self) -> usize {
                0
            }
        }

        let parked = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let dispatcher = Arc::new(ParkDispatcher {
            parked: Arc::clone(&parked),
            peak: Arc::clone(&peak),
            release: Arc::clone(&release),
        });
        let handler = Arc::new(NextBridgeHandler::with_inflight_limit(dispatcher, Some(2)));
        let mut joins = Vec::new();
        for _ in 0..8 {
            let h = Arc::clone(&handler);
            joins.push(tokio::spawn(async move {
                let req = Request::builder()
                    .uri("/")
                    .body(Body::empty())
                    .expect("request");
                h.handle(req).await.expect("infallible")
            }));
        }
        // Let tasks attempt to enter dispatch.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            peak.load(Ordering::SeqCst) <= 2,
            "peak concurrent dispatches must not exceed permit cap"
        );
        // Drain all 8 by releasing 8 permits to the dispatcher gate.
        release.add_permits(8);
        for j in joins {
            j.await.expect("task");
        }
        assert!(
            peak.load(Ordering::SeqCst) <= 2,
            "peak after drain must still respect the cap"
        );
    }

    #[tokio::test]
    async fn inflight_semaphore_clamps_zero_to_one() {
        let dispatcher = Arc::new(EchoDispatcher {
            count: Arc::new(AtomicUsize::new(0)),
        });
        let handler = NextBridgeHandler::with_inflight_limit(Arc::clone(&dispatcher), Some(0));
        let req = Request::builder()
            .uri("/")
            .body(Body::from("ok"))
            .expect("request");
        let response = handler.handle(req).await.expect("infallible");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(dispatcher.dispatch_count(), 1);
    }
}
