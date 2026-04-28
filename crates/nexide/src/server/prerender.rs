//! Hot path for prerendered Next.js pages and RSC payloads.
//!
//! `next build` writes prerendered routes into
//! `.next/server/app/<path>.{html,rsc,meta}`. Every request that maps
//! to one of those files can be answered without ever touching the V8
//! isolate, dropping TTFB from ~20ms (full dispatch) down to a single
//! `fs::metadata` call plus a memcpy from the in-process cache.
//!
//! Freshness is validated via `mtime + size` - when ISR regenerates a
//! prerender (writing a new `.html`/`.meta` pair on disk), the next
//! request observes the change and reloads the cache entry.

use std::collections::HashMap;
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Instant, SystemTime};

use axum::body::Body;
use axum::http::header::{HeaderName, HeaderValue};
use axum::http::{Method, Request, Response, StatusCode};
use bytes::Bytes;
use serde::Deserialize;
use sha1::{Digest, Sha1};
use tower::Service;
use tower::service_fn;
use tower::util::BoxCloneSyncService;

use super::static_assets::DynamicService;

/// Soft upper bound on cache entries. Reached only by pathological
/// apps with thousands of prerendered routes; on overflow we drop the
/// table wholesale (cheap, all entries are reload-on-demand).
const CACHE_CAPACITY: usize = 4096;

/// Service alias mirroring [`DynamicService`] so the router can chain
/// `ServeDir → Prerender → Dynamic` without naming concrete futures.
pub(super) type PrerenderService = BoxCloneSyncService<Request<Body>, Response<Body>, Infallible>;

/// Builds a hot-path service that serves prerendered pages out of
/// `app_dir` (typically `<standalone>/.next/server/app`) and forwards
/// everything else to `fallback` (the V8 dynamic handler).
///
/// The returned service is `Clone + Send + Sync` and safe to share
/// across the Tokio worker pool.
pub(super) fn prerender_with_fallback(
    app_dir: PathBuf,
    fallback: DynamicService,
) -> PrerenderService {
    let inner = Arc::new(PrerenderInner::new(app_dir));
    let svc = service_fn(move |req: Request<Body>| {
        let inner = inner.clone();
        let mut fallback = fallback.clone();
        async move {
            let started = Instant::now();
            if let Some(mut response) = try_serve(&inner, &req) {
                stamp_server_timing(response.headers_mut(), "rust-hot", started.elapsed());
                return Ok::<_, Infallible>(response);
            }
            let mut response = fallback.call(req).await?;
            stamp_server_timing(response.headers_mut(), "v8-dispatch", started.elapsed());
            Ok::<_, Infallible>(response)
        }
    });
    BoxCloneSyncService::new(svc)
}

/// Shared mutable state guarded by [`prerender_with_fallback`].
struct PrerenderInner {
    root: PathBuf,
    cache: RwLock<HashMap<String, CachedAsset>>,
}

impl PrerenderInner {
    fn new(root: PathBuf) -> Self {
        Self {
            root,
            cache: RwLock::new(HashMap::with_capacity(64)),
        }
    }
}

/// Stamps `Server-Timing` on the outbound response. The canonical
/// total metric (`srv;desc="…";dur=ms`) is **prepended** to whatever
/// per-phase breakdown a downstream handler may have appended (see
/// [`crate::server::next_bridge::NextBridgeHandler`]) so the browser's
/// `parseServerTiming` regex picks the total first while devtools and
/// `curl -v` still see the diagnostic breakdown.
///
/// Also stamps `Timing-Allow-Origin: *` so cross-origin probes can
/// observe the header through `PerformanceResourceTiming`.
fn stamp_server_timing(
    headers: &mut axum::http::HeaderMap,
    desc: &'static str,
    elapsed: std::time::Duration,
) {
    let micros = u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX);
    #[allow(clippy::cast_precision_loss)]
    let ms = micros as f64 / 1000.0;
    let mut value = format!("srv;desc=\"{desc}\";dur={ms:.3}");
    let existing = headers
        .get("server-timing")
        .and_then(|v| v.to_str().ok())
        .map(ToOwned::to_owned);
    if let Some(rest) = existing
        && !rest.is_empty()
    {
        value.push_str(", ");
        value.push_str(&rest);
    }
    if let Ok(v) = HeaderValue::from_str(&value) {
        headers.insert("server-timing", v);
    }
    headers.insert("timing-allow-origin", HeaderValue::from_static("*"));
}

/// Cached payload plus the metadata required to detect ISR rewrites.
#[derive(Clone)]
struct CachedAsset {
    bytes: Bytes,
    etag: String,
    content_type: &'static str,
    extra_headers: Vec<(HeaderName, HeaderValue)>,
    mtime: SystemTime,
    size: u64,
}

/// Subset of the `.meta` sidecar that affects HTTP responses.
#[derive(Deserialize)]
struct AssetMeta {
    #[serde(default)]
    headers: HashMap<String, String>,
}

/// Inspects `req` and returns a fully-formed response when the path
/// matches a prerendered asset, or `None` to delegate to the fallback.
fn try_serve(inner: &PrerenderInner, req: &Request<Body>) -> Option<Response<Body>> {
    if !matches!(*req.method(), Method::GET | Method::HEAD) {
        return None;
    }
    let is_rsc = req.headers().get("rsc").is_some_and(|v| v == "1");
    let key = lookup_key(req.uri().path(), is_rsc)?;
    let asset = resolve_asset(inner, &key, is_rsc)?;
    Some(build_response(&asset, req.method() == Method::HEAD))
}

/// Translates a request path into the relative file path that Next.js
/// writes under `.next/server/app/`. Returns `None` for inputs that
/// cannot map to a prerender (path traversal, non-printable bytes,
/// reserved Next.js prefixes).
fn lookup_key(path: &str, is_rsc: bool) -> Option<String> {
    if path.starts_with("/_next/") || path.starts_with("/api/") {
        return None;
    }
    let trimmed = path.trim_start_matches('/').trim_end_matches('/');
    if trimmed.contains("..") || trimmed.contains('\\') {
        return None;
    }
    let stem = if trimmed.is_empty() { "index" } else { trimmed };
    let suffix = if is_rsc { ".rsc" } else { ".html" };
    Some(format!("{stem}{suffix}"))
}

/// Cache lookup with mtime/size revalidation. On miss or staleness,
/// reloads from disk and inserts the fresh entry.
fn resolve_asset(inner: &PrerenderInner, key: &str, is_rsc: bool) -> Option<CachedAsset> {
    if let Ok(guard) = inner.cache.read()
        && let Some(hit) = guard.get(key)
        && file_matches(&inner.root, key, hit)
    {
        return Some(hit.clone());
    }
    let fresh = load_asset(&inner.root, key, is_rsc)?;
    if let Ok(mut guard) = inner.cache.write() {
        if guard.len() >= CACHE_CAPACITY {
            guard.clear();
        }
        guard.insert(key.to_owned(), fresh.clone());
    }
    Some(fresh)
}

/// Returns `true` when the file at `<root>/<key>` matches the cached
/// entry's mtime + size - i.e. the cache is still authoritative.
fn file_matches(root: &Path, key: &str, asset: &CachedAsset) -> bool {
    let Ok(meta) = std::fs::metadata(root.join(key)) else {
        return false;
    };
    if meta.len() != asset.size {
        return false;
    }
    meta.modified().map(|m| m == asset.mtime).unwrap_or(false)
}

/// Loads `<root>/<key>` and its `.meta` sidecar (if present) into a
/// freshly built [`CachedAsset`]. Returns `None` when the payload is
/// missing - the request will fall through to the dynamic handler.
fn load_asset(root: &Path, key: &str, is_rsc: bool) -> Option<CachedAsset> {
    let payload_path = root.join(key);
    let meta = std::fs::metadata(&payload_path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let bytes = Bytes::from(std::fs::read(&payload_path).ok()?);
    let mtime = meta.modified().ok()?;
    let extra_headers = load_meta_headers(root, key);
    let etag = compute_etag(&bytes);
    let content_type = if is_rsc {
        "text/x-component"
    } else {
        "text/html; charset=utf-8"
    };
    Some(CachedAsset {
        bytes,
        etag,
        content_type,
        extra_headers,
        mtime,
        size: meta.len(),
    })
}

/// Reads the `<key-without-suffix>.meta` JSON sidecar and converts
/// recognized header keys into `(HeaderName, HeaderValue)` pairs.
/// Unknown / malformed keys are silently dropped - Next.js owns this
/// schema and we never want a bad sidecar to break the hot path.
fn load_meta_headers(root: &Path, key: &str) -> Vec<(HeaderName, HeaderValue)> {
    let Some((stem, _ext)) = key.rsplit_once('.') else {
        return Vec::new();
    };
    let meta_path = root.join(format!("{stem}.meta"));
    let Ok(raw) = std::fs::read(&meta_path) else {
        return Vec::new();
    };
    let Ok(parsed): Result<AssetMeta, _> = serde_json::from_slice(&raw) else {
        return Vec::new();
    };
    parsed
        .headers
        .into_iter()
        .filter_map(|(k, v)| {
            let name = HeaderName::try_from(k).ok()?;
            let value = HeaderValue::try_from(v).ok()?;
            Some((name, value))
        })
        .collect()
}

/// Stable content-addressed `ETag` (SHA-1, base16 truncated to 13
/// chars - same width Next.js produces). Quoting matches RFC 7232
/// strong-validator syntax.
fn compute_etag(bytes: &[u8]) -> String {
    let digest = Sha1::digest(bytes);
    let hex = format!("{digest:x}");
    let short: String = hex.chars().take(13).collect();
    format!("\"{short}\"")
}

/// Assembles the outbound HTTP response. `head_only` returns the
/// header set with an empty body (HEAD requests / 304s out-of-scope
/// for MVP).
fn build_response(asset: &CachedAsset, head_only: bool) -> Response<Body> {
    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header("content-type", asset.content_type)
        .header("etag", asset.etag.as_str())
        .header("vary", "rsc, next-router-state-tree, next-router-prefetch")
        .header("cache-control", "s-maxage=31536000")
        .header("x-nextjs-cache", "HIT")
        .header("content-length", asset.size.to_string());
    for (name, value) in &asset.extra_headers {
        builder = builder.header(name.clone(), value.clone());
    }
    let body = if head_only {
        Body::empty()
    } else {
        Body::from(asset.bytes.clone())
    };
    builder
        .body(body)
        .expect("static response builder cannot fail")
}

#[cfg(test)]
mod tests {
    use super::{
        CachedAsset, compute_etag, file_matches, load_asset, lookup_key, prerender_with_fallback,
    };
    use crate::server::fallback::NotImplementedHandler;
    use crate::server::static_assets::dynamic_service;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use bytes::Bytes;
    use http_body_util::BodyExt;
    use std::sync::Arc;
    use std::time::SystemTime;
    use tempfile::TempDir;
    use tower::ServiceExt;

    fn write_prerender(dir: &std::path::Path, route: &str, html: &str, meta: Option<&str>) {
        let html_path = dir.join(format!("{route}.html"));
        if let Some(parent) = html_path.parent() {
            std::fs::create_dir_all(parent).expect("create_dir_all");
        }
        std::fs::write(&html_path, html).expect("write html");
        if let Some(meta_body) = meta {
            std::fs::write(dir.join(format!("{route}.meta")), meta_body).expect("write meta");
        }
    }

    #[test]
    fn lookup_key_maps_root_to_index_html() {
        assert_eq!(lookup_key("/", false).as_deref(), Some("index.html"));
        assert_eq!(lookup_key("", false).as_deref(), Some("index.html"));
    }

    #[test]
    fn lookup_key_strips_trailing_slash_and_handles_segments() {
        assert_eq!(lookup_key("/about", false).as_deref(), Some("about.html"));
        assert_eq!(lookup_key("/about/", false).as_deref(), Some("about.html"));
        assert_eq!(
            lookup_key("/users/1", false).as_deref(),
            Some("users/1.html"),
        );
    }

    #[test]
    fn lookup_key_emits_rsc_suffix_when_requested() {
        assert_eq!(lookup_key("/about", true).as_deref(), Some("about.rsc"));
        assert_eq!(lookup_key("/", true).as_deref(), Some("index.rsc"));
    }

    #[test]
    fn lookup_key_rejects_reserved_prefixes() {
        assert!(lookup_key("/_next/static/foo.js", false).is_none());
        assert!(lookup_key("/api/echo", false).is_none());
    }

    #[test]
    fn lookup_key_blocks_path_traversal() {
        assert!(lookup_key("/../etc/passwd", false).is_none());
        assert!(lookup_key("/foo/..", false).is_none());
        assert!(lookup_key("/foo\\bar", false).is_none());
    }

    #[test]
    fn etag_is_stable_and_quoted() {
        let a = compute_etag(b"hello world");
        let b = compute_etag(b"hello world");
        let c = compute_etag(b"goodbye");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.starts_with('"') && a.ends_with('"'));
    }

    #[test]
    fn load_asset_returns_none_for_missing_payload() {
        let tmp = TempDir::new().expect("tempdir");
        assert!(load_asset(tmp.path(), "missing.html", false).is_none());
    }

    #[test]
    fn load_asset_reads_payload_and_meta() {
        let tmp = TempDir::new().expect("tempdir");
        write_prerender(
            tmp.path(),
            "about",
            "<html>about</html>",
            Some(r#"{"headers":{"x-nextjs-prerender":"1"}}"#),
        );
        let asset = load_asset(tmp.path(), "about.html", false).expect("asset");
        assert_eq!(asset.bytes.as_ref(), b"<html>about</html>");
        assert_eq!(asset.content_type, "text/html; charset=utf-8");
        assert!(
            asset
                .extra_headers
                .iter()
                .any(|(k, _)| k.as_str() == "x-nextjs-prerender"),
        );
    }

    #[test]
    fn file_matches_detects_rewrites() {
        let tmp = TempDir::new().expect("tempdir");
        write_prerender(tmp.path(), "stale", "<html>v1</html>", None);
        let asset = load_asset(tmp.path(), "stale.html", false).expect("asset");
        assert!(file_matches(tmp.path(), "stale.html", &asset));

        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(tmp.path().join("stale.html"), b"<html>v2-longer</html>").expect("rewrite");
        assert!(!file_matches(tmp.path(), "stale.html", &asset));
    }

    #[test]
    fn file_matches_returns_false_when_payload_disappeared() {
        let asset = CachedAsset {
            bytes: Bytes::from_static(b"x"),
            etag: "\"deadbeef\"".into(),
            content_type: "text/html; charset=utf-8",
            extra_headers: Vec::new(),
            mtime: SystemTime::UNIX_EPOCH,
            size: 1,
        };
        let tmp = TempDir::new().expect("tempdir");
        assert!(!file_matches(tmp.path(), "ghost.html", &asset));
    }

    #[tokio::test]
    async fn service_serves_prerendered_html_with_meta_headers() {
        let tmp = TempDir::new().expect("tempdir");
        write_prerender(
            tmp.path(),
            "about",
            "<html>about</html>",
            Some(r#"{"headers":{"x-nextjs-prerender":"1"}}"#),
        );
        let svc = prerender_with_fallback(
            tmp.path().to_path_buf(),
            dynamic_service(Arc::new(NotImplementedHandler)),
        );
        let resp = svc
            .oneshot(
                Request::builder()
                    .uri("/about")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("infallible");
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get("content-type")
                .map(|h| h.to_str().unwrap()),
            Some("text/html; charset=utf-8"),
        );
        assert_eq!(
            resp.headers()
                .get("x-nextjs-cache")
                .map(|h| h.to_str().unwrap()),
            Some("HIT"),
        );
        assert_eq!(
            resp.headers()
                .get("x-nextjs-prerender")
                .map(|h| h.to_str().unwrap()),
            Some("1"),
        );
        let body = resp.into_body().collect().await.expect("body").to_bytes();
        assert_eq!(body.as_ref(), b"<html>about</html>");
    }

    #[tokio::test]
    async fn service_falls_back_for_dynamic_routes() {
        let tmp = TempDir::new().expect("tempdir");
        let svc = prerender_with_fallback(
            tmp.path().to_path_buf(),
            dynamic_service(Arc::new(NotImplementedHandler)),
        );
        let resp = svc
            .oneshot(
                Request::builder()
                    .uri("/api/echo")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("infallible");
        assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
    }

    #[tokio::test]
    async fn service_falls_back_for_post_even_when_prerender_exists() {
        let tmp = TempDir::new().expect("tempdir");
        write_prerender(tmp.path(), "about", "<html>about</html>", None);
        let svc = prerender_with_fallback(
            tmp.path().to_path_buf(),
            dynamic_service(Arc::new(NotImplementedHandler)),
        );
        let resp = svc
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/about")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("infallible");
        assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
    }

    #[tokio::test]
    async fn service_serves_rsc_when_header_present() {
        let tmp = TempDir::new().expect("tempdir");
        std::fs::write(tmp.path().join("about.rsc"), b"RSC payload").expect("write");
        let svc = prerender_with_fallback(
            tmp.path().to_path_buf(),
            dynamic_service(Arc::new(NotImplementedHandler)),
        );
        let resp = svc
            .oneshot(
                Request::builder()
                    .uri("/about")
                    .header("rsc", "1")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("infallible");
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get("content-type")
                .map(|h| h.to_str().unwrap()),
            Some("text/x-component"),
        );
    }

    #[tokio::test]
    async fn head_request_omits_body() {
        let tmp = TempDir::new().expect("tempdir");
        write_prerender(tmp.path(), "about", "<html>about</html>", None);
        let svc = prerender_with_fallback(
            tmp.path().to_path_buf(),
            dynamic_service(Arc::new(NotImplementedHandler)),
        );
        let resp = svc
            .oneshot(
                Request::builder()
                    .method("HEAD")
                    .uri("/about")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("infallible");
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get("content-length")
                .map(|h| h.to_str().unwrap()),
            Some("18"),
        );
        let body = resp.into_body().collect().await.expect("body").to_bytes();
        assert!(body.is_empty());
    }
}
