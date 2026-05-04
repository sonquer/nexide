//! In-RAM cache for `/_next/static/*` immutable assets.
//!
//! Wraps an inner [`tower::Service`] (typically a [`ServeDir`]) and
//! caches successful (`200 OK`) responses keyed by request path. Since
//! Next.js content-hashes every chunk under `/_next/static`, paths are
//! safely treated as immutable — no mtime validation needed.
//!
//! On cache hit the service:
//!   * skips the disk syscall path entirely (no `open`/`fstat`/`read`);
//!   * picks the best precomputed encoding from `Accept-Encoding`
//!     (`br q11` > `gzip 9` > `identity`) and stamps `Content-Encoding`
//!     so the outer [`CompressionLayer`] does not re-compress.
//!
//! [`ServeDir`]: tower_http::services::ServeDir
//! [`CompressionLayer`]: tower_http::compression::CompressionLayer

use std::collections::HashMap;
use std::convert::Infallible;
use std::env;
use std::future::Future;
use std::io::Write;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::Instant;

use axum::body::Body;
use axum::http::header::{
    ACCEPT_ENCODING, CACHE_CONTROL, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, ETAG,
    HeaderName, LAST_MODIFIED, VARY,
};
use axum::http::{HeaderMap, HeaderValue, Request, Response, StatusCode};
use brotli::enc::BrotliEncoderParams;
use bytes::Bytes;
use http_body_util::BodyExt;
use parking_lot::Mutex;
use tower::Service;

const VARY_ACCEPT_ENCODING: HeaderValue = HeaderValue::from_static("accept-encoding");
const ENCODING_BR: HeaderValue = HeaderValue::from_static("br");
const ENCODING_GZIP: HeaderValue = HeaderValue::from_static("gzip");
const HN_X_NEXIDE_STATIC: HeaderName = HeaderName::from_static("x-nexide-static-cache");
const HV_HIT: HeaderValue = HeaderValue::from_static("HIT");
const HV_MISS: HeaderValue = HeaderValue::from_static("MISS");

const DEFAULT_CACHE_MB: u64 = 64;
const MAX_ENTRY_BYTES: u64 = 8 * 1024 * 1024;
const MIN_COMPRESS_BYTES: usize = 256;

#[derive(Clone)]
struct CachedAsset {
    identity: Bytes,
    br_q11: Option<Bytes>,
    gzip9: Option<Bytes>,
    content_type: Option<HeaderValue>,
    etag: Option<HeaderValue>,
    last_modified: Option<HeaderValue>,
    cache_control: Option<HeaderValue>,
}

impl CachedAsset {
    fn footprint(&self) -> u64 {
        let mut bytes = self.identity.len() as u64;
        if let Some(b) = &self.br_q11 {
            bytes += b.len() as u64;
        }
        if let Some(g) = &self.gzip9 {
            bytes += g.len() as u64;
        }
        bytes
    }
}

#[derive(Default)]
struct Inner {
    entries: HashMap<String, Arc<CachedAsset>>,
    order: Vec<String>,
    bytes: u64,
}

/// Shared state of the RAM cache.
pub(super) struct RamCacheState {
    inner: Mutex<Inner>,
    cap_bytes: u64,
    pub hits: AtomicU64,
    pub misses: AtomicU64,
    pub stored_bytes: AtomicU64,
}

impl RamCacheState {
    fn new(cap_bytes: u64) -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            cap_bytes,
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            stored_bytes: AtomicU64::new(0),
        }
    }

    fn shrink(&self) {
        let mut inner = self.inner.lock();
        inner.entries.clear();
        inner.order.clear();
        inner.bytes = 0;
        self.stored_bytes.store(0, Ordering::Relaxed);
    }

    fn lookup(&self, key: &str) -> Option<Arc<CachedAsset>> {
        let inner = match self.inner.try_lock() {
            Some(g) => {
                crate::diagnostics::contention::record_fast(
                    &crate::diagnostics::contention::RAM_CACHE_FAST,
                );
                g
            }
            None => {
                crate::diagnostics::contention::record_contended(
                    &crate::diagnostics::contention::RAM_CACHE_CONTENDED,
                );
                self.inner.lock()
            }
        };
        inner.entries.get(key).cloned()
    }

    fn touch(&self, key: &str) {
        let mut inner = match self.inner.try_lock() {
            Some(g) => {
                crate::diagnostics::contention::record_fast(
                    &crate::diagnostics::contention::RAM_CACHE_FAST,
                );
                g
            }
            None => {
                crate::diagnostics::contention::record_contended(
                    &crate::diagnostics::contention::RAM_CACHE_CONTENDED,
                );
                self.inner.lock()
            }
        };
        if let Some(idx) = inner.order.iter().position(|k| k == key) {
            let k = inner.order.remove(idx);
            inner.order.push(k);
        }
    }

    fn insert(&self, key: String, asset: Arc<CachedAsset>) {
        let footprint = asset.footprint();
        if footprint > self.cap_bytes {
            return;
        }
        let mut inner = match self.inner.try_lock() {
            Some(g) => {
                crate::diagnostics::contention::record_fast(
                    &crate::diagnostics::contention::RAM_CACHE_FAST,
                );
                g
            }
            None => {
                crate::diagnostics::contention::record_contended(
                    &crate::diagnostics::contention::RAM_CACHE_CONTENDED,
                );
                self.inner.lock()
            }
        };
        if let Some(prev) = inner.entries.remove(&key) {
            inner.bytes = inner.bytes.saturating_sub(prev.footprint());
            if let Some(idx) = inner.order.iter().position(|k| k == &key) {
                inner.order.remove(idx);
            }
        }
        while inner.bytes + footprint > self.cap_bytes {
            let Some(victim) = inner.order.first().cloned() else {
                break;
            };
            inner.order.remove(0);
            if let Some(prev) = inner.entries.remove(&victim) {
                inner.bytes = inner.bytes.saturating_sub(prev.footprint());
            }
        }
        inner.bytes += footprint;
        inner.order.push(key.clone());
        inner.entries.insert(key, asset);
        self.stored_bytes.store(inner.bytes, Ordering::Relaxed);
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.inner.lock().entries.len()
    }

    #[cfg(test)]
    fn current_bytes(&self) -> u64 {
        self.inner.lock().bytes
    }
}

fn cache_capacity_bytes() -> u64 {
    let mb = env::var("NEXIDE_STATIC_RAM_MB")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_CACHE_MB);
    mb.saturating_mul(1024 * 1024)
}

/// A Tower service that wraps an inner static-file service with a
/// content-immutable RAM cache.
pub(super) struct RamCachedService<S> {
    inner: S,
    state: Arc<RamCacheState>,
}

impl<S: Clone> Clone for RamCachedService<S> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            state: self.state.clone(),
        }
    }
}

impl<S> RamCachedService<S> {
    pub(super) fn new(inner: S) -> Self {
        Self::with_capacity(inner, cache_capacity_bytes())
    }

    pub(super) fn with_capacity(inner: S, cap_bytes: u64) -> Self {
        let state = Arc::new(RamCacheState::new(cap_bytes));
        let weak = Arc::downgrade(&state);
        crate::pool::idle_shrink::register(move || {
            if let Some(strong) = weak.upgrade() {
                strong.shrink();
            }
        });
        Self { inner, state }
    }

    #[cfg(test)]
    pub(super) fn state(&self) -> Arc<RamCacheState> {
        self.state.clone()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Encoding {
    Br,
    Gzip,
    Identity,
}

fn pick_encoding(headers: &HeaderMap) -> Encoding {
    let Some(value) = headers.get(ACCEPT_ENCODING) else {
        return Encoding::Identity;
    };
    let Ok(text) = value.to_str() else {
        return Encoding::Identity;
    };
    let lower = text.to_ascii_lowercase();
    if accepts(&lower, "br") {
        Encoding::Br
    } else if accepts(&lower, "gzip") {
        Encoding::Gzip
    } else {
        Encoding::Identity
    }
}

fn accepts(header: &str, token: &str) -> bool {
    header.split(',').any(|part| {
        let part = part.trim();
        let name = part.split(';').next().unwrap_or("").trim();
        if name != token {
            return false;
        }
        for attr in part.split(';').skip(1) {
            let attr = attr.trim();
            if let Some(rest) = attr.strip_prefix("q=") {
                if let Ok(q) = rest.parse::<f32>() {
                    return q > 0.0;
                }
                return true;
            }
        }
        true
    })
}

fn brotli_q11(data: &[u8]) -> Option<Bytes> {
    let params = BrotliEncoderParams {
        quality: 11,
        ..Default::default()
    };
    let mut out = Vec::with_capacity(data.len() / 2);
    let mut cursor = data;
    if brotli::BrotliCompress(&mut cursor, &mut out, &params).is_err() {
        return None;
    }
    if out.len() >= data.len() {
        return None;
    }
    Some(Bytes::from(out))
}

fn gzip_q9(data: &[u8]) -> Option<Bytes> {
    let mut encoder = flate2::write::GzEncoder::new(
        Vec::with_capacity(data.len() / 2),
        flate2::Compression::new(9),
    );
    if encoder.write_all(data).is_err() {
        return None;
    }
    let out = match encoder.finish() {
        Ok(v) => v,
        Err(_) => return None,
    };
    if out.len() >= data.len() {
        return None;
    }
    Some(Bytes::from(out))
}

fn should_compress(content_type: Option<&HeaderValue>, len: usize) -> bool {
    if len < MIN_COMPRESS_BYTES {
        return false;
    }
    let Some(ct) = content_type else {
        return false;
    };
    let Ok(s) = ct.to_str() else {
        return false;
    };
    let s = s.to_ascii_lowercase();
    if s.starts_with("image/")
        || s.starts_with("video/")
        || s.starts_with("audio/")
        || s.starts_with("font/woff2")
        || s == "application/wasm"
        || s == "application/zip"
        || s == "application/octet-stream"
    {
        return false;
    }
    true
}

fn build_response_from_cache(asset: &CachedAsset, encoding: Encoding) -> Response<Body> {
    let (body_bytes, encoding_header) = match encoding {
        Encoding::Br => match &asset.br_q11 {
            Some(b) => (b.clone(), Some(ENCODING_BR)),
            None => (asset.identity.clone(), None),
        },
        Encoding::Gzip => match &asset.gzip9 {
            Some(g) => (g.clone(), Some(ENCODING_GZIP)),
            None => (asset.identity.clone(), None),
        },
        Encoding::Identity => (asset.identity.clone(), None),
    };
    let len = body_bytes.len() as u64;
    let mut resp = Response::new(Body::from(body_bytes));
    *resp.status_mut() = StatusCode::OK;
    let headers = resp.headers_mut();
    if let Some(ct) = &asset.content_type {
        headers.insert(CONTENT_TYPE, ct.clone());
    }
    if let Some(etag) = &asset.etag {
        headers.insert(ETAG, etag.clone());
    }
    if let Some(lm) = &asset.last_modified {
        headers.insert(LAST_MODIFIED, lm.clone());
    }
    if let Some(cc) = &asset.cache_control {
        headers.insert(CACHE_CONTROL, cc.clone());
    }
    headers.insert(CONTENT_LENGTH, HeaderValue::from(len));
    headers.insert(VARY, VARY_ACCEPT_ENCODING);
    if let Some(enc) = encoding_header {
        headers.insert(CONTENT_ENCODING, enc);
    }
    headers.insert(HN_X_NEXIDE_STATIC, HV_HIT);
    resp
}

impl<S, B> Service<Request<Body>> for RamCachedService<S>
where
    S: Service<Request<Body>, Response = Response<B>, Error = Infallible> + Clone + Send + 'static,
    S::Future: Send + 'static,
    B: http_body::Body<Data = Bytes> + Send + 'static,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>> + Send + Sync,
{
    type Response = Response<Body>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Response<Body>, Infallible>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        let state = self.state.clone();
        let mut inner = self.inner.clone();
        std::mem::swap(&mut inner, &mut self.inner);
        let key = req.uri().path().to_string();
        let encoding = pick_encoding(req.headers());
        Box::pin(async move {
            if let Some(asset) = state.lookup(&key) {
                state.touch(&key);
                state.hits.fetch_add(1, Ordering::Relaxed);
                return Ok(build_response_from_cache(&asset, encoding));
            }
            state.misses.fetch_add(1, Ordering::Relaxed);
            let started = Instant::now();
            let response = inner.call(req).await?;
            let elapsed = started.elapsed();
            if response.status() != StatusCode::OK {
                let (parts, body) = response.into_parts();
                return Ok(Response::from_parts(parts, Body::new(body)));
            }
            let (parts, body) = response.into_parts();
            if parts.headers.get(CONTENT_ENCODING).is_some() {
                return Ok(Response::from_parts(parts, Body::new(body)));
            }
            let collected = match body.collect().await {
                Ok(c) => c.to_bytes(),
                Err(_) => {
                    let mut resp = Response::new(Body::empty());
                    *resp.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
                    return Ok(resp);
                }
            };
            if (collected.len() as u64) > MAX_ENTRY_BYTES {
                let len = collected.len() as u64;
                let mut resp = Response::from_parts(parts, Body::from(collected));
                resp.headers_mut()
                    .insert(CONTENT_LENGTH, HeaderValue::from(len));
                resp.headers_mut().insert(HN_X_NEXIDE_STATIC, HV_MISS);
                return Ok(resp);
            }
            let content_type = parts.headers.get(CONTENT_TYPE).cloned();
            let etag = parts.headers.get(ETAG).cloned();
            let last_modified = parts.headers.get(LAST_MODIFIED).cloned();
            let cache_control = parts.headers.get(CACHE_CONTROL).cloned();
            let bytes = collected.clone();
            let (br_q11, gzip9) = if should_compress(content_type.as_ref(), bytes.len()) {
                let bytes_for_br = bytes.clone();
                let bytes_for_gz = bytes.clone();
                let br_handle = tokio::task::spawn_blocking(move || brotli_q11(&bytes_for_br));
                let gz_handle = tokio::task::spawn_blocking(move || gzip_q9(&bytes_for_gz));
                let br = br_handle.await.ok().flatten();
                let gz = gz_handle.await.ok().flatten();
                (br, gz)
            } else {
                (None, None)
            };
            let asset = Arc::new(CachedAsset {
                identity: bytes.clone(),
                br_q11,
                gzip9,
                content_type,
                etag,
                last_modified,
                cache_control,
            });
            state.insert(key, asset.clone());
            tracing::trace!(
                bytes = bytes.len(),
                br = asset.br_q11.as_ref().map(|b| b.len()).unwrap_or(0),
                gz = asset.gzip9.as_ref().map(|g| g.len()).unwrap_or(0),
                miss_ms = elapsed.as_millis() as u64,
                "static ram cache populate"
            );
            Ok(build_response_from_cache(&asset, encoding))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::static_assets::next_static_only;
    use http_body_util::BodyExt;
    use tempfile::TempDir;
    use tower::ServiceExt;

    fn make_request(path: &str, accept_encoding: Option<&str>) -> Request<Body> {
        let mut b = Request::builder().uri(path);
        if let Some(ae) = accept_encoding {
            b = b.header(ACCEPT_ENCODING, ae);
        }
        b.body(Body::empty()).expect("request")
    }

    #[test]
    fn pick_encoding_prefers_br() {
        let mut h = HeaderMap::new();
        h.insert(
            ACCEPT_ENCODING,
            HeaderValue::from_static("gzip, deflate, br"),
        );
        assert_eq!(pick_encoding(&h), Encoding::Br);
    }

    #[test]
    fn pick_encoding_falls_back_to_gzip() {
        let mut h = HeaderMap::new();
        h.insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip, deflate"));
        assert_eq!(pick_encoding(&h), Encoding::Gzip);
    }

    #[test]
    fn pick_encoding_identity_when_q_zero() {
        let mut h = HeaderMap::new();
        h.insert(
            ACCEPT_ENCODING,
            HeaderValue::from_static("br;q=0, gzip;q=0"),
        );
        assert_eq!(pick_encoding(&h), Encoding::Identity);
    }

    #[tokio::test]
    async fn cache_miss_then_hit_serves_identity() {
        let tmp = TempDir::new().expect("tempdir");
        let payload = "x".repeat(2048);
        std::fs::write(tmp.path().join("chunk.js"), &payload).expect("write");
        let svc = RamCachedService::with_capacity(next_static_only(tmp.path()), 1 << 20);
        let state = svc.state();

        let resp = svc
            .clone()
            .oneshot(make_request("/chunk.js", None))
            .await
            .expect("infallible");
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.expect("body").to_bytes();
        assert_eq!(body.as_ref(), payload.as_bytes());
        assert_eq!(state.misses.load(Ordering::Relaxed), 1);
        assert_eq!(state.hits.load(Ordering::Relaxed), 0);
        assert_eq!(state.len(), 1);

        let resp2 = svc
            .clone()
            .oneshot(make_request("/chunk.js", None))
            .await
            .expect("infallible");
        assert_eq!(resp2.status(), StatusCode::OK);
        assert_eq!(
            resp2.headers().get(HN_X_NEXIDE_STATIC).expect("header"),
            HV_HIT
        );
        assert_eq!(state.hits.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn cache_hit_returns_brotli_body() {
        let tmp = TempDir::new().expect("tempdir");
        let payload = "console.log('hello world');".repeat(200);
        std::fs::write(tmp.path().join("app.js"), &payload).expect("write");
        let svc = RamCachedService::with_capacity(next_static_only(tmp.path()), 1 << 20);

        let _ = svc
            .clone()
            .oneshot(make_request("/app.js", None))
            .await
            .expect("infallible");
        let resp = svc
            .clone()
            .oneshot(make_request("/app.js", Some("br")))
            .await
            .expect("infallible");
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get(CONTENT_ENCODING).expect("encoding"),
            ENCODING_BR
        );
        let body = resp.into_body().collect().await.expect("body").to_bytes();
        assert!(body.len() < payload.len());
    }

    #[tokio::test]
    async fn cache_hit_returns_gzip_body_when_br_unavailable() {
        let tmp = TempDir::new().expect("tempdir");
        let payload = "abc".repeat(4096);
        std::fs::write(tmp.path().join("g.js"), &payload).expect("write");
        let svc = RamCachedService::with_capacity(next_static_only(tmp.path()), 1 << 20);
        let _ = svc
            .clone()
            .oneshot(make_request("/g.js", None))
            .await
            .expect("infallible");
        let resp = svc
            .clone()
            .oneshot(make_request("/g.js", Some("gzip")))
            .await
            .expect("infallible");
        assert_eq!(
            resp.headers().get(CONTENT_ENCODING).expect("encoding"),
            ENCODING_GZIP
        );
    }

    #[tokio::test]
    async fn lru_evicts_oldest_under_pressure() {
        let tmp = TempDir::new().expect("tempdir");
        let payload = "y".repeat(4 * 1024);
        std::fs::write(tmp.path().join("a.js"), &payload).expect("write");
        std::fs::write(tmp.path().join("b.js"), &payload).expect("write");
        std::fs::write(tmp.path().join("c.js"), &payload).expect("write");
        let svc = RamCachedService::with_capacity(next_static_only(tmp.path()), 8 * 1024);
        let state = svc.state();
        for n in ["/a.js", "/b.js", "/c.js"] {
            let _ = svc
                .clone()
                .oneshot(make_request(n, None))
                .await
                .expect("ok");
        }
        assert!(state.current_bytes() <= 8 * 1024);
        assert!(state.len() < 3);
    }

    #[tokio::test]
    async fn missing_path_passes_through() {
        let tmp = TempDir::new().expect("tempdir");
        let svc = RamCachedService::with_capacity(next_static_only(tmp.path()), 1 << 20);
        let state = svc.state();
        let resp = svc
            .clone()
            .oneshot(make_request("/nope.js", None))
            .await
            .expect("ok");
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        assert_eq!(state.len(), 0);
    }

    #[tokio::test]
    async fn small_body_is_not_compressed() {
        let tmp = TempDir::new().expect("tempdir");
        std::fs::write(tmp.path().join("tiny.js"), "ok").expect("write");
        let svc = RamCachedService::with_capacity(next_static_only(tmp.path()), 1 << 20);
        let _ = svc
            .clone()
            .oneshot(make_request("/tiny.js", None))
            .await
            .expect("ok");
        let resp = svc
            .clone()
            .oneshot(make_request("/tiny.js", Some("br")))
            .await
            .expect("ok");
        assert!(resp.headers().get(CONTENT_ENCODING).is_none());
    }
}
