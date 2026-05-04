//! Axum handler for `/_next/image`.
//!
//! Implements the upstream Next.js image-optimizer route end-to-end in
//! Rust: query validation, source resolution (local/remote allowlists),
//! magic-byte content-type sniffing, format negotiation, resize,
//! re-encode, and disk caching. Bypassed source types (SVG, ICO, BMP,
//! JXL, HEIC) and animated images are served unchanged. The pipeline
//! never enters V8.

use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::http::header::{
    ACCEPT, CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_SECURITY_POLICY,
    CONTENT_TYPE, ETAG, IF_NONE_MATCH, VARY,
};
use axum::http::{HeaderMap, HeaderValue, Method, Request, Response, StatusCode};
use bytes::Bytes;
use tower::service_fn;
use tower::util::BoxCloneSyncService;
use tracing::{debug, warn};

use super::cache::{self, CacheEntry};
use super::config::ImageConfig;
use super::glob::{local_pattern_matches, remote_pattern_matches};
use super::pipeline::{self, OutputFormat, SourceFormat};

const MAX_URL_LENGTH: usize = 3072;
const X_NEXTJS_CACHE: &str = "x-nextjs-cache";

/// Concrete service type produced by [`next_image_service`].
pub(crate) type NextImageService = BoxCloneSyncService<Request<Body>, Response<Body>, Infallible>;

struct Ctx {
    app_dir: PathBuf,
    public_dir: PathBuf,
    next_static_dir: PathBuf,
    bind_addr: std::net::SocketAddr,
    config: ImageConfig,
    http: reqwest::Client,
    mem: super::memory::MemCache,
    dynamic: Option<Arc<dyn crate::server::fallback::DynamicHandler>>,
}

/// Builds the `/_next/image` handler service.
#[must_use]
pub fn next_image_service(
    app_dir: PathBuf,
    public_dir: PathBuf,
    next_static_dir: PathBuf,
    bind_addr: std::net::SocketAddr,
) -> NextImageService {
    next_image_service_with_dynamic(app_dir, public_dir, next_static_dir, bind_addr, None)
}

/// Same as [`next_image_service`] but also receives the dynamic
/// (Next.js bridge) handler so internal `/_next/image?url=/api/...`
/// fetches resolve in-process instead of looping through TCP. Avoids
/// connection-pool deadlocks and the 7-second timeout penalty when
/// the Next bridge is busy.
#[must_use]
pub fn next_image_service_with_dynamic(
    app_dir: PathBuf,
    public_dir: PathBuf,
    next_static_dir: PathBuf,
    bind_addr: std::net::SocketAddr,
    dynamic: Option<Arc<dyn crate::server::fallback::DynamicHandler>>,
) -> NextImageService {
    let config = ImageConfig::from_app_dir(&app_dir);
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(7))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap_or_default();
    let ctx = Arc::new(Ctx {
        app_dir,
        public_dir,
        next_static_dir,
        bind_addr,
        config,
        http,
        mem: super::memory::MemCache::new(),
        dynamic,
    });
    let svc = service_fn(move |req: Request<Body>| {
        let ctx = ctx.clone();
        async move { Ok::<_, Infallible>(handle(&ctx, req).await.unwrap_or_else(error_response)) }
    });
    BoxCloneSyncService::new(svc)
}

#[derive(Debug)]
struct HandlerError {
    status: StatusCode,
    message: String,
}

impl HandlerError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}

async fn handle(ctx: &Arc<Ctx>, req: Request<Body>) -> Result<Response<Body>, HandlerError> {
    if !matches!(req.method(), &Method::GET | &Method::HEAD) {
        return Err(HandlerError::new(
            StatusCode::METHOD_NOT_ALLOWED,
            "method not allowed",
        ));
    }
    if !ctx.config.route_enabled() {
        return Err(HandlerError::new(StatusCode::NOT_FOUND, "not found"));
    }

    let query = req.uri().query().unwrap_or("");
    let params = ValidatedParams::parse(query, &ctx.config)?;

    if params.url.contains("/_next/image") {
        return Err(HandlerError::new(
            StatusCode::BAD_REQUEST,
            "\"url\" parameter cannot be recursive",
        ));
    }

    let accept = req
        .headers()
        .get(ACCEPT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let if_none_match = req
        .headers()
        .get(IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim_matches('"').to_owned());

    if let Some(provisional) = preselect_output(&accept, &ctx.config.formats) {
        let key = cache::cache_key(
            &params.url,
            params.width,
            params.quality,
            provisional.mime(),
        );
        if let Some(hot) = ctx.mem.get(&key) {
            let now = current_unix_ms();
            let cache_state = if hot.expire_at_ms > now {
                "HIT"
            } else {
                "STALE"
            };
            return Ok(serve_hot(
                &hot,
                provisional,
                cache_state,
                &if_none_match,
                &params.url,
                &ctx.config,
            ));
        }
    }

    let source = resolve_source(ctx, &params, &accept).await?;
    let detected = pipeline::detect_format(&source.bytes);
    if !detected.is_image() {
        return Err(HandlerError::new(
            StatusCode::BAD_REQUEST,
            "The requested resource isn't a valid image.",
        ));
    }
    if matches!(detected, SourceFormat::Svg) && !ctx.config.dangerously_allow_svg {
        return Err(HandlerError::new(
            StatusCode::BAD_REQUEST,
            "\"url\" parameter is valid but image type is not allowed",
        ));
    }

    let max_age = ctx.config.minimum_cache_ttl.max(source.upstream_max_age);

    if detected.is_bypass() || matches!(detected, SourceFormat::Svg) {
        return Ok(serve_bypass(ctx, &params, &source, detected, max_age));
    }

    let chosen = pipeline::choose_output(detected, &accept, &ctx.config.formats);
    let key = cache::cache_key(&params.url, params.width, params.quality, chosen.mime());
    let upstream_etag = cache::encode_upstream_etag(&source.upstream_etag);

    if let Some(hot) = ctx.mem.get(&key) {
        let now = current_unix_ms();
        let cache_state = if hot.expire_at_ms > now {
            "HIT"
        } else {
            "STALE"
        };
        return Ok(serve_hot(
            &hot,
            chosen,
            cache_state,
            &if_none_match,
            &params.url,
            &ctx.config,
        ));
    }

    if let Some(hit) = cache::read(&ctx.app_dir, &key) {
        let now = current_unix_ms();
        let cache_state = if hit.expire_at_ms > now {
            "HIT"
        } else {
            "STALE"
        };
        let hot = Arc::new(super::memory::HotEntry::from_disk(
            &hit,
            chosen.mime(),
            &params.url,
            &ctx.config,
        ));
        ctx.mem.put(key.clone(), Arc::clone(&hot));
        return Ok(serve_hot(
            &hot,
            chosen,
            cache_state,
            &if_none_match,
            &params.url,
            &ctx.config,
        ));
    }

    let bytes_owned = source.bytes.clone();
    let url_owned = params.url.clone();
    let width = params.width;
    let quality = params.quality;
    let (tx, rx) = tokio::sync::oneshot::channel();
    rayon::spawn(move || {
        let _ = tx.send(produce_optimized(&bytes_owned, width, quality, chosen));
    });
    let optimized = rx.await.map_err(|_| {
        HandlerError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "image pipeline join failed",
        )
    })??;

    let etag = cache::buffer_etag(&optimized);
    let expire_at_ms = current_unix_ms().saturating_add(u128::from(max_age) * 1000);
    let entry = CacheEntry {
        key: key.clone(),
        max_age,
        expire_at_ms,
        etag: etag.clone(),
        upstream_etag,
        extension: cache::extension_for(chosen),
        bytes: optimized,
    };
    if let Err(err) = cache::write(&ctx.app_dir, &entry) {
        warn!(target: "nexide::image", error = %err, "cache write failed");
    } else {
        debug!(target: "nexide::image", url = %url_owned, "cached optimized image");
    }
    let hot = Arc::new(super::memory::HotEntry::from_disk(
        &entry,
        chosen.mime(),
        &params.url,
        &ctx.config,
    ));
    ctx.mem.put(key, Arc::clone(&hot));

    Ok(serve_hot(
        &hot,
        chosen,
        "MISS",
        &if_none_match,
        &params.url,
        &ctx.config,
    ))
}

fn preselect_output(accept: &str, formats: &[String]) -> Option<OutputFormat> {
    if accept.is_empty() {
        return None;
    }
    for f in formats {
        if accept_contains_mime(accept, f)
            && let Some(fmt) = OutputFormat::from_mime(f)
        {
            return Some(fmt);
        }
    }
    None
}

fn accept_contains_mime(accept: &str, mime: &str) -> bool {
    accept
        .split(',')
        .map(|tok| tok.split(';').next().unwrap_or("").trim())
        .any(|t| t.eq_ignore_ascii_case(mime))
}

fn produce_optimized(
    src: &[u8],
    width: u32,
    quality: u8,
    chosen: OutputFormat,
) -> Result<Vec<u8>, HandlerError> {
    let img = pipeline::decode(src).map_err(|_| {
        HandlerError::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Unable to optimize image and unable to fallback to upstream image",
        )
    })?;
    let resized = pipeline::resize(&img, width).map_err(|_| {
        HandlerError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Unable to optimize image and unable to fallback to upstream image",
        )
    })?;
    pipeline::encode(&resized, chosen, quality).map_err(|_| {
        HandlerError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Unable to optimize image and unable to fallback to upstream image",
        )
    })
}

fn serve_hot(
    entry: &super::memory::HotEntry,
    chosen: OutputFormat,
    cache_state: &'static str,
    if_none_match: &Option<String>,
    _url: &str,
    cfg: &ImageConfig,
) -> Response<Body> {
    if let Some(client_etag) = if_none_match
        && client_etag.trim_matches('"') == entry.etag
    {
        let mut resp = Response::new(Body::empty());
        *resp.status_mut() = StatusCode::NOT_MODIFIED;
        attach_headers_from_hot(resp.headers_mut(), chosen.mime(), cache_state, entry, cfg);
        return resp;
    }
    let len = entry.bytes.len();
    let mut resp = Response::new(Body::from(entry.bytes.clone()));
    attach_headers_from_hot(resp.headers_mut(), chosen.mime(), cache_state, entry, cfg);
    resp.headers_mut()
        .insert(CONTENT_LENGTH, HeaderValue::from(len as u64));
    resp
}

fn serve_bypass(
    _ctx: &Arc<Ctx>,
    params: &ValidatedParams,
    source: &Source,
    fmt: SourceFormat,
    max_age: u64,
) -> Response<Body> {
    let mime = fmt.mime();
    let etag = cache::buffer_etag(&source.bytes);
    let bytes = Bytes::from(source.bytes.clone());
    let len = bytes.len();
    let mut resp = Response::new(Body::from(bytes));
    attach_headers(
        resp.headers_mut(),
        mime,
        max_age,
        &etag,
        "MISS",
        &params.url,
        &source.config_snapshot,
    );
    resp.headers_mut()
        .insert(CONTENT_LENGTH, HeaderValue::from(len as u64));
    resp
}

fn attach_headers(
    headers: &mut HeaderMap,
    mime: &'static str,
    max_age: u64,
    etag: &str,
    cache_state: &'static str,
    url: &str,
    cfg: &ImageConfig,
) {
    const HV_VARY_ACCEPT: HeaderValue = HeaderValue::from_static("Accept");
    const HN_X_NEXTJS_CACHE_K: axum::http::HeaderName =
        axum::http::HeaderName::from_static(X_NEXTJS_CACHE);
    headers.insert(VARY, HV_VARY_ACCEPT);
    headers.insert(CONTENT_TYPE, HeaderValue::from_static(mime));
    if let Ok(v) = HeaderValue::from_str(&format!("public, max-age={max_age}, must-revalidate")) {
        headers.insert(CACHE_CONTROL, v);
    }
    if let Ok(v) = HeaderValue::from_str(&format!("\"{etag}\"")) {
        headers.insert(ETAG, v);
    }
    headers.insert(HN_X_NEXTJS_CACHE_K, HeaderValue::from_static(cache_state));
    if let Ok(v) = HeaderValue::from_str(&cfg.content_security_policy) {
        headers.insert(CONTENT_SECURITY_POLICY, v);
    }
    let disposition = build_content_disposition(url, mime, &cfg.content_disposition_type);
    if let Ok(v) = HeaderValue::from_str(&disposition) {
        headers.insert(CONTENT_DISPOSITION, v);
    }
}

fn attach_headers_from_hot(
    headers: &mut HeaderMap,
    mime: &'static str,
    cache_state: &'static str,
    entry: &super::memory::HotEntry,
    cfg: &ImageConfig,
) {
    const HV_VARY_ACCEPT: HeaderValue = HeaderValue::from_static("Accept");
    const HN_X_NEXTJS_CACHE_K: axum::http::HeaderName =
        axum::http::HeaderName::from_static(X_NEXTJS_CACHE);
    headers.insert(VARY, HV_VARY_ACCEPT);
    headers.insert(CONTENT_TYPE, HeaderValue::from_static(mime));
    headers.insert(CACHE_CONTROL, entry.cache_control_hv.clone());
    headers.insert(ETAG, entry.etag_hv.clone());
    headers.insert(HN_X_NEXTJS_CACHE_K, HeaderValue::from_static(cache_state));
    if let Ok(v) = HeaderValue::from_str(&cfg.content_security_policy) {
        headers.insert(CONTENT_SECURITY_POLICY, v);
    }
    headers.insert(CONTENT_DISPOSITION, entry.disposition_hv.clone());
}

pub(super) fn build_content_disposition(url: &str, mime: &str, disposition_type: &str) -> String {
    let filename = filename_from_url(url, mime);
    format!("{disposition_type}; filename=\"{filename}\"")
}

fn filename_from_url(url: &str, mime: &str) -> String {
    let path = url.split('?').next().unwrap_or(url);
    let stem = path.rsplit('/').next().unwrap_or("image");
    let stem = if stem.is_empty() { "image" } else { stem };
    if stem.contains('.') {
        return sanitize(stem);
    }
    let ext = match mime {
        "image/webp" => "webp",
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/svg+xml" => "svg",
        _ => "img",
    };
    sanitize(&format!("{stem}.{ext}"))
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c == '"' || c.is_control() { '_' } else { c })
        .collect()
}

fn current_unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default()
}

#[derive(Debug)]
struct ValidatedParams {
    url: String,
    width: u32,
    quality: u8,
}

impl ValidatedParams {
    fn parse(query: &str, cfg: &ImageConfig) -> Result<Self, HandlerError> {
        let mut url: Option<String> = None;
        let mut url_count = 0u32;
        let mut width_raw: Option<String> = None;
        let mut width_count = 0u32;
        let mut quality_raw: Option<String> = None;
        let mut quality_count = 0u32;

        for pair in query.split('&') {
            if pair.is_empty() {
                continue;
            }
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            let decoded = percent_decode(v);
            match k {
                "url" => {
                    url_count += 1;
                    url = Some(decoded);
                }
                "w" => {
                    width_count += 1;
                    width_raw = Some(decoded);
                }
                "q" => {
                    quality_count += 1;
                    quality_raw = Some(decoded);
                }
                _ => {}
            }
        }

        if url_count > 1 {
            return Err(bad_request("\"url\" parameter cannot be an array"));
        }
        let url = url.ok_or_else(|| bad_request("\"url\" parameter is required"))?;
        if url.is_empty() {
            return Err(bad_request("\"url\" parameter is required"));
        }
        if url.len() > MAX_URL_LENGTH {
            return Err(bad_request("\"url\" parameter is too long"));
        }
        if url.starts_with("//") {
            return Err(bad_request(
                "\"url\" parameter cannot be a protocol-relative URL (//)",
            ));
        }

        if width_count > 1 {
            return Err(bad_request("\"w\" parameter (width) cannot be an array"));
        }
        let width_str =
            width_raw.ok_or_else(|| bad_request("\"w\" parameter (width) is required"))?;
        if !width_str.chars().all(|c| c.is_ascii_digit()) || width_str.is_empty() {
            return Err(bad_request(
                "\"w\" parameter (width) must be an integer greater than 0",
            ));
        }
        let width: u32 = width_str.parse().map_err(|_| {
            bad_request("\"w\" parameter (width) must be an integer greater than 0")
        })?;
        if width == 0 {
            return Err(bad_request(
                "\"w\" parameter (width) must be an integer greater than 0",
            ));
        }
        if !cfg.allows_width(width) {
            return Err(bad_request(format!(
                "\"w\" parameter (width) of {width} is not allowed"
            )));
        }

        if quality_count > 1 {
            return Err(bad_request("\"q\" parameter (quality) cannot be an array"));
        }
        let quality_str =
            quality_raw.ok_or_else(|| bad_request("\"q\" parameter (quality) is required"))?;
        if !quality_str.chars().all(|c| c.is_ascii_digit()) || quality_str.is_empty() {
            return Err(bad_request(
                "\"q\" parameter (quality) must be an integer between 1 and 100",
            ));
        }
        let quality_num: u32 = quality_str.parse().map_err(|_| {
            bad_request("\"q\" parameter (quality) must be an integer between 1 and 100")
        })?;
        if !(1..=100).contains(&quality_num) {
            return Err(bad_request(
                "\"q\" parameter (quality) must be an integer between 1 and 100",
            ));
        }
        let quality = u8::try_from(quality_num).unwrap_or(75);
        if !cfg.allows_quality(quality) {
            return Err(bad_request(format!(
                "\"q\" parameter (quality) of {quality} is not allowed"
            )));
        }

        Ok(Self {
            url,
            width,
            quality,
        })
    }
}

fn bad_request(msg: impl Into<String>) -> HandlerError {
    HandlerError::new(StatusCode::BAD_REQUEST, msg)
}

struct Source {
    bytes: Vec<u8>,
    upstream_etag: String,
    upstream_max_age: u64,
    config_snapshot: ImageConfig,
}

async fn resolve_source(
    ctx: &Arc<Ctx>,
    params: &ValidatedParams,
    accept: &str,
) -> Result<Source, HandlerError> {
    if params.url.starts_with("http://") || params.url.starts_with("https://") {
        return fetch_remote(ctx, &params.url).await;
    }
    if !params.url.starts_with('/') {
        return Err(bad_request("\"url\" parameter is invalid"));
    }
    let parsed = url::Url::parse(&format!("http://n{}", params.url))
        .map_err(|_| bad_request("\"url\" parameter is invalid"))?;
    let path = parsed.path();
    let search = parsed.query().unwrap_or("");
    if !local_pattern_matches(&ctx.config.local_patterns, path, search) {
        return Err(bad_request("\"url\" parameter is not allowed"));
    }

    // 1. /_next/static/* → resolve directly from the build static dir.
    if let Some(rel) = path.strip_prefix("/_next/static/") {
        let candidate = ctx.next_static_dir.join(rel);
        let canonical_root = ctx
            .next_static_dir
            .canonicalize()
            .unwrap_or_else(|_| ctx.next_static_dir.clone());
        let canonical_candidate = candidate
            .canonicalize()
            .unwrap_or_else(|_| candidate.clone());
        if !canonical_candidate.starts_with(&canonical_root) {
            return Err(bad_request("\"url\" parameter is not allowed"));
        }
        if let Ok(bytes) = std::fs::read(&canonical_candidate) {
            return Ok(Source {
                bytes,
                upstream_etag: String::new(),
                upstream_max_age: 0,
                config_snapshot: ctx.config.clone(),
            });
        }
        return Err(HandlerError::new(StatusCode::NOT_FOUND, "source not found"));
    }

    // 2. public/ on disk: prefer zero-cost filesystem read when present.
    let rel = path.trim_start_matches('/');
    let candidate = ctx.public_dir.join(rel);
    if candidate.is_file() {
        let canonical_root = ctx
            .public_dir
            .canonicalize()
            .unwrap_or_else(|_| ctx.public_dir.clone());
        let canonical_candidate = candidate
            .canonicalize()
            .unwrap_or_else(|_| candidate.clone());
        if !canonical_candidate.starts_with(&canonical_root) {
            return Err(bad_request("\"url\" parameter is not allowed"));
        }
        if let Ok(bytes) = std::fs::read(&canonical_candidate) {
            return Ok(Source {
                bytes,
                upstream_etag: String::new(),
                upstream_max_age: 0,
                config_snapshot: ctx.config.clone(),
            });
        }
    }

    // 3. Otherwise, treat as an internal Next.js route (e.g. `/api/...`,
    //    rewrites, dynamic handlers) and fetch back through the same
    //    HTTP shield - mirrors `fetchInternalImage` in upstream
    //    `next/dist/server/image-optimizer.js`.
    fetch_internal(ctx, &params.url, accept).await
}

async fn fetch_internal(ctx: &Arc<Ctx>, href: &str, accept: &str) -> Result<Source, HandlerError> {
    if let Some(handler) = ctx.dynamic.as_ref() {
        return fetch_via_handler(ctx, handler.as_ref(), href, accept).await;
    }
    fetch_via_loopback(ctx, href, accept).await
}

async fn fetch_via_handler(
    ctx: &Arc<Ctx>,
    handler: &dyn crate::server::fallback::DynamicHandler,
    href: &str,
    accept: &str,
) -> Result<Source, HandlerError> {
    let mut builder = Request::builder().method(Method::GET).uri(href).header(
        axum::http::header::USER_AGENT,
        "nexide-image-optimizer/1 (internal)",
    );
    if !accept.is_empty() {
        builder = builder.header(ACCEPT, accept);
    }
    let req = builder.body(Body::empty()).map_err(|err| {
        warn!(target: "nexide::image", url = %href, error = %err, "internal request build failed");
        HandlerError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "\"url\" parameter is valid but upstream response is invalid",
        )
    })?;
    let resp = handler.handle(req).await.map_err(|err| {
        warn!(target: "nexide::image", url = %href, error = %err, "internal handler failed");
        HandlerError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "\"url\" parameter is valid but upstream response is invalid",
        )
    })?;
    let status = resp.status();
    if !status.is_success() {
        let mapped = if status == StatusCode::NOT_FOUND {
            StatusCode::NOT_FOUND
        } else {
            StatusCode::from_u16(508).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
        };
        return Err(HandlerError::new(
            mapped,
            "\"url\" parameter is valid but upstream response is invalid",
        ));
    }
    let upstream_etag = resp
        .headers()
        .get(ETAG)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim_matches('"').to_owned())
        .unwrap_or_default();
    let upstream_max_age = resp
        .headers()
        .get(CACHE_CONTROL)
        .and_then(|v| v.to_str().ok())
        .map(parse_cache_control_max_age)
        .unwrap_or(0);
    let limit = ctx.config.maximum_response_body;
    let bytes_full = match axum::body::to_bytes(resp.into_body(), limit as usize).await {
        Ok(b) => b,
        Err(err) => {
            warn!(target: "nexide::image", error = %err, "internal handler body read failed");
            return Err(HandlerError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "\"url\" parameter is valid but upstream response is invalid",
            ));
        }
    };
    if (bytes_full.len() as u64) > limit {
        return Err(HandlerError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "\"url\" parameter is valid but upstream response is too large",
        ));
    }
    Ok(Source {
        bytes: bytes_full.to_vec(),
        upstream_etag,
        upstream_max_age,
        config_snapshot: ctx.config.clone(),
    })
}

async fn fetch_via_loopback(
    ctx: &Arc<Ctx>,
    href: &str,
    accept: &str,
) -> Result<Source, HandlerError> {
    let host = match ctx.bind_addr {
        std::net::SocketAddr::V4(v4) => {
            let ip = v4.ip();
            if ip.is_unspecified() {
                "127.0.0.1".to_owned()
            } else {
                ip.to_string()
            }
        }
        std::net::SocketAddr::V6(v6) => {
            let ip = v6.ip();
            if ip.is_unspecified() {
                "[::1]".to_owned()
            } else {
                format!("[{ip}]")
            }
        }
    };
    let url = format!("http://{}:{}{}", host, ctx.bind_addr.port(), href);
    let mut req = ctx.http.get(&url);
    if !accept.is_empty() {
        req = req.header(reqwest::header::ACCEPT, accept);
    }
    req = req.header(
        reqwest::header::USER_AGENT,
        "nexide-image-optimizer/1 (internal)",
    );
    let resp = req.send().await.map_err(|err| {
        warn!(target: "nexide::image", url = %url, error = %err, "internal fetch failed");
        HandlerError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "\"url\" parameter is valid but upstream response is invalid",
        )
    })?;
    let status = resp.status();
    if !status.is_success() {
        let mapped = if status == reqwest::StatusCode::NOT_FOUND {
            StatusCode::NOT_FOUND
        } else {
            StatusCode::from_u16(508).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
        };
        return Err(HandlerError::new(
            mapped,
            "\"url\" parameter is valid but upstream response is invalid",
        ));
    }
    let upstream_etag = resp
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim_matches('"').to_owned())
        .unwrap_or_default();
    let upstream_max_age = resp
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|v| v.to_str().ok())
        .map(parse_cache_control_max_age)
        .unwrap_or(0);
    let limit = ctx.config.maximum_response_body;
    let bytes_full = resp.bytes().await.map_err(|err| {
        warn!(target: "nexide::image", error = %err, "internal fetch body read failed");
        HandlerError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "\"url\" parameter is valid but upstream response is invalid",
        )
    })?;
    if (bytes_full.len() as u64) > limit {
        return Err(HandlerError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "\"url\" parameter is valid but upstream response is too large",
        ));
    }
    Ok(Source {
        bytes: bytes_full.to_vec(),
        upstream_etag,
        upstream_max_age,
        config_snapshot: ctx.config.clone(),
    })
}

async fn fetch_remote(ctx: &Arc<Ctx>, href: &str) -> Result<Source, HandlerError> {
    let parsed = url::Url::parse(href).map_err(|_| bad_request("\"url\" parameter is invalid"))?;
    if !remote_pattern_matches(&ctx.config.remote_patterns, &ctx.config.domains, &parsed) {
        return Err(bad_request("\"url\" parameter is not allowed"));
    }
    let mut current = parsed.clone();
    let mut redirects_left = ctx.config.maximum_redirects;
    loop {
        let resp = ctx.http.get(current.as_str()).send().await.map_err(|err| {
            if err.is_timeout() {
                HandlerError::new(
                    StatusCode::GATEWAY_TIMEOUT,
                    "\"url\" parameter is valid but upstream response timed out",
                )
            } else {
                HandlerError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "\"url\" parameter is valid but upstream response is invalid",
                )
            }
        })?;
        let status = resp.status();
        if matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308) {
            let location = resp
                .headers()
                .get(axum::http::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            let Some(loc) = location else {
                return Err(HandlerError::new(
                    StatusCode::from_u16(508).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                    "\"url\" parameter is valid but upstream response is invalid",
                ));
            };
            if redirects_left == 0 {
                return Err(HandlerError::new(
                    StatusCode::from_u16(508).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                    "\"url\" parameter is valid but upstream response is invalid",
                ));
            }
            current = current.join(&loc).map_err(|_| {
                HandlerError::new(
                    StatusCode::from_u16(508).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                    "\"url\" parameter is valid but upstream response is invalid",
                )
            })?;
            redirects_left -= 1;
            continue;
        }
        if !status.is_success() {
            return Err(HandlerError {
                status,
                message: "\"url\" parameter is valid but upstream response is invalid".to_owned(),
            });
        }
        let upstream_etag = resp
            .headers()
            .get(axum::http::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned();
        let upstream_max_age = resp
            .headers()
            .get(axum::http::header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok())
            .map(parse_cache_control_max_age)
            .unwrap_or(0);

        let max = ctx.config.maximum_response_body;
        if let Some(cl) = resp.content_length()
            && cl > max
        {
            return Err(HandlerError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "\"url\" parameter is valid but upstream response is invalid",
            ));
        }
        let body = resp.bytes().await.map_err(|_| {
            HandlerError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "\"url\" parameter is valid but upstream response is invalid",
            )
        })?;
        if (body.len() as u64) > max {
            return Err(HandlerError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "\"url\" parameter is valid but upstream response is invalid",
            ));
        }
        let buf = body.to_vec();
        if buf.is_empty() {
            return Err(bad_request(
                "\"url\" parameter is valid but upstream response is invalid",
            ));
        }
        return Ok(Source {
            bytes: buf,
            upstream_etag,
            upstream_max_age,
            config_snapshot: ctx.config.clone(),
        });
    }
}

fn parse_cache_control_max_age(header: &str) -> u64 {
    let mut s_maxage: Option<u64> = None;
    let mut max_age: Option<u64> = None;
    for token in header.split(',') {
        let token = token.trim();
        if let Some(v) = strip_directive(token, "s-maxage") {
            s_maxage = v.parse::<u64>().ok();
        } else if let Some(v) = strip_directive(token, "max-age") {
            max_age = v.parse::<u64>().ok();
        }
    }
    s_maxage.or(max_age).unwrap_or(0)
}

fn strip_directive<'a>(token: &'a str, directive: &str) -> Option<&'a str> {
    let token = token.trim();
    let lower = token.to_ascii_lowercase();
    let prefix = format!("{directive}=");
    if !lower.starts_with(&prefix) {
        return None;
    }
    let value = &token[prefix.len()..];
    Some(value.trim_matches('"'))
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'+' {
            out.push(b' ');
            i += 1;
            continue;
        }
        if c == b'%'
            && i + 2 < bytes.len()
            && let (Some(h), Some(l)) = (hex_digit(bytes[i + 1]), hex_digit(bytes[i + 2]))
        {
            out.push((h << 4) | l);
            i += 3;
            continue;
        }
        out.push(c);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

const fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn error_response(err: HandlerError) -> Response<Body> {
    let mut resp = Response::new(Body::from(err.message));
    *resp.status_mut() = err.status;
    resp.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    resp
}
