//! Static-asset serving layer.
//!
//! Backed by [`tower_http::services::ServeDir`], which on Linux uses
//! `sendfile(2)` so disk bytes go straight from the page cache into
//! the socket without bouncing through user space (zero-copy).

use std::convert::Infallible;
use std::path::Path;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use tower::service_fn;
use tower::util::BoxCloneSyncService;
use tower_http::services::ServeDir;

use super::fallback::DynamicHandler;

/// Concrete service type produced by [`dynamic_service`]. Aliased so
/// that downstream `ServeDir<DynamicService>` has a stable, nameable
/// signature.
pub(super) type DynamicService = BoxCloneSyncService<Request<Body>, Response<Body>, Infallible>;

/// Type returned by [`public_with_fallback_service`].
///
/// `ServeDir::fallback` keeps the inner service's response intact
/// (unlike `not_found_service`, which forces HTTP 404). This lets the
/// fallback chain return arbitrary status codes - e.g. 200 for SSR
/// hits, 501 while the bridge is unimplemented, 502 on engine errors.
pub(super) type PublicService = ServeDir<DynamicService>;

/// Builds the service serving `public/` with a fallback to the supplied
/// downstream service (typically prerender hot-path → dynamic).
pub(super) fn public_with_fallback_service(
    public_dir: &Path,
    fallback: DynamicService,
) -> PublicService {
    ServeDir::new(public_dir)
        .call_fallback_on_method_not_allowed(true)
        .fallback(fallback)
}

/// Builds the service serving `_next/static/` chunks only.
///
/// No fallback is wired here on purpose - the chunk set is fully
/// determined at build time, so a missing chunk is a build error
/// rather than an SSR concern.
pub(super) fn next_static_only(next_static_dir: &Path) -> ServeDir {
    ServeDir::new(next_static_dir)
}

/// Wraps a [`DynamicHandler`] into a clonable Tower service usable
/// inside [`ServeDir::not_found_service`].
pub(super) fn dynamic_service(handler: Arc<dyn DynamicHandler>) -> DynamicService {
    let inner = service_fn(move |req: Request<Body>| {
        let handler = handler.clone();
        async move {
            let request_headers = req.headers().clone();
            let response = match handler.handle(req).await {
                Ok(response) => response,
                Err(error) => bad_gateway(&error.to_string(), Some(&request_headers)),
            };
            Ok::<_, Infallible>(response)
        }
    });
    BoxCloneSyncService::new(inner)
}

fn bad_gateway(message: &str, request_headers: Option<&axum::http::HeaderMap>) -> Response<Body> {
    super::error_page::render(StatusCode::BAD_GATEWAY, request_headers, Some(message))
}

#[cfg(test)]
mod tests {
    use super::{bad_gateway, dynamic_service, next_static_only, public_with_fallback_service};
    use crate::server::fallback::{DynamicHandler, HandlerError, NotImplementedHandler};
    use axum::body::Body;
    use axum::http::{Request, Response, StatusCode};
    use http_body_util::BodyExt;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tower::ServiceExt;

    #[test]
    fn bad_gateway_carries_status() {
        let response = bad_gateway("boom", None);
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    }

    #[tokio::test]
    async fn dynamic_service_routes_to_handler() {
        let service = dynamic_service(Arc::new(NotImplementedHandler));
        let response = service
            .oneshot(
                Request::builder()
                    .uri("/anything")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("infallible");
        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    }

    #[tokio::test]
    async fn public_serves_existing_file() {
        let tmp = TempDir::new().expect("tempdir");
        std::fs::write(tmp.path().join("hello.txt"), "world").expect("write");
        let svc = public_with_fallback_service(
            tmp.path(),
            dynamic_service(Arc::new(NotImplementedHandler)),
        );
        let response = svc
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
    async fn public_falls_back_to_dynamic_for_missing_file() {
        let tmp = TempDir::new().expect("tempdir");
        let svc = public_with_fallback_service(
            tmp.path(),
            dynamic_service(Arc::new(NotImplementedHandler)),
        );
        let response = svc
            .oneshot(
                Request::builder()
                    .uri("/nope")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("infallible");
        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    }

    #[tokio::test]
    async fn next_static_serves_chunks_only() {
        let tmp = TempDir::new().expect("tempdir");
        std::fs::write(tmp.path().join("chunk.js"), "// chunk").expect("write");
        let svc = next_static_only(tmp.path());
        let response = svc
            .oneshot(
                Request::builder()
                    .uri("/chunk.js")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("infallible");
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[derive(Default)]
    struct ExplodingHandler;

    #[async_trait::async_trait]
    impl DynamicHandler for ExplodingHandler {
        async fn handle(&self, _req: Request<Body>) -> Result<Response<Body>, HandlerError> {
            Err(HandlerError::Upstream("kaboom".into()))
        }
    }

    #[tokio::test]
    async fn dynamic_service_translates_error_to_502() {
        let service = dynamic_service(Arc::new(ExplodingHandler));
        let response = service
            .oneshot(
                Request::builder()
                    .uri("/x")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("infallible");
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    }
}
