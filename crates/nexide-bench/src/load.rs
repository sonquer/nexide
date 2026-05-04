//! Async HTTP load generator. Drives N parallel virtual users hitting
//! a single URL for a fixed wall-clock window and records per-request
//! latency into an HDR histogram for accurate tail percentiles.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use hdrhistogram::Histogram;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use tokio::sync::Mutex;

/// Specification of a single load run: target URL plus traffic shape.
#[derive(Debug, Clone)]
pub struct LoadSpec {
    /// Fully-qualified URL to hit (e.g. `http://127.0.0.1:3000/api/ping`).
    pub url: String,
    /// Number of concurrent virtual users.
    pub connections: usize,
    /// Total duration of the measurement window.
    pub duration: Duration,
    /// Extra headers to send with every request.
    pub headers: Vec<(String, String)>,
}

/// Aggregated outcome of a single load run.
#[derive(Debug, Clone)]
pub struct LoadOutcome {
    /// Successful HTTP responses (`2xx`).
    pub ok: u64,
    /// Non-2xx responses + transport-level errors (sum of the buckets
    /// below; kept for backwards-compat with table renderers).
    pub errors: u64,
    /// Responses where the server replied with a non-2xx HTTP status.
    /// A high count here means the server is *up* and answering, just
    /// rejecting (5xx, 4xx) — typically queue-saturation / handler
    /// crash mid-request.
    pub http_errors: u64,
    /// Transport-level failures: connection refused, RST, EOF before
    /// response, or any other I/O error returned by the HTTP client.
    /// A high count here means the server is *not accepting* / *not
    /// responding* — TCP backlog overflow, listener crash, or runtime
    /// hang. When this dominates, the bench should bail and dump
    /// `docker logs`.
    pub transport_errors: u64,
    /// Subset of `transport_errors` where the underlying error was
    /// the reqwest 10s per-request timeout. A high count here means
    /// the server *accepts* but *never replies* — handler deadlock,
    /// dispatcher queue stall, or V8 isolate stuck in GC.
    pub timeout_errors: u64,
    /// Total wall-clock time observed by the harness.
    pub elapsed: Duration,
    /// Median latency.
    pub p50: Duration,
    /// 95th percentile latency.
    pub p95: Duration,
    /// 99th percentile latency.
    pub p99: Duration,
    /// Throughput in requests per second (ok responses only).
    pub rps: f64,
}

fn build_headers(spec: &LoadSpec) -> Result<HeaderMap> {
    let mut map = HeaderMap::new();
    for (name, value) in &spec.headers {
        let name = HeaderName::from_bytes(name.as_bytes()).context("header name")?;
        let value = HeaderValue::from_str(value).context("header value")?;
        map.insert(name, value);
    }
    Ok(map)
}

/// Drive `spec.connections` workers against `spec.url` for
/// `spec.duration` and return the aggregated load profile.
///
/// # Errors
/// Returns an error when the HTTP client cannot be built or when the
/// request headers are malformed.
pub async fn run_load(spec: LoadSpec) -> Result<LoadOutcome> {
    let headers = build_headers(&spec)?;
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(spec.connections)
        .timeout(Duration::from_secs(10))
        .default_headers(headers)
        .build()
        .context("reqwest client")?;
    let histogram = Arc::new(Mutex::new(Histogram::<u64>::new_with_max(60_000_000, 3)?));
    let ok = Arc::new(AtomicU64::new(0));
    let http_err = Arc::new(AtomicU64::new(0));
    let transport_err = Arc::new(AtomicU64::new(0));
    let timeout_err = Arc::new(AtomicU64::new(0));
    let deadline = Instant::now() + spec.duration;
    let mut tasks = Vec::with_capacity(spec.connections);
    for _ in 0..spec.connections {
        let client = client.clone();
        let url = spec.url.clone();
        let histogram = Arc::clone(&histogram);
        let ok = Arc::clone(&ok);
        let http_err = Arc::clone(&http_err);
        let transport_err = Arc::clone(&transport_err);
        let timeout_err = Arc::clone(&timeout_err);
        tasks.push(tokio::spawn(async move {
            while Instant::now() < deadline {
                let started = Instant::now();
                match client.get(&url).send().await {
                    Ok(resp) => {
                        let status_ok = resp.status().is_success();
                        let _ = resp.bytes().await;
                        let micros =
                            u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
                        let mut h = histogram.lock().await;
                        h.record(micros).ok();
                        drop(h);
                        if status_ok {
                            ok.fetch_add(1, Ordering::Relaxed);
                        } else {
                            http_err.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    Err(err) => {
                        if err.is_timeout() {
                            timeout_err.fetch_add(1, Ordering::Relaxed);
                        }
                        transport_err.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }));
    }
    let started_at = Instant::now();
    for task in tasks {
        let _ = task.await;
    }
    let elapsed = started_at.elapsed();
    let h = histogram.lock().await;
    let p50 = Duration::from_micros(h.value_at_quantile(0.50));
    let p95 = Duration::from_micros(h.value_at_quantile(0.95));
    let p99 = Duration::from_micros(h.value_at_quantile(0.99));
    drop(h);
    let ok_total = ok.load(Ordering::Relaxed);
    let http_err_total = http_err.load(Ordering::Relaxed);
    let transport_err_total = transport_err.load(Ordering::Relaxed);
    let timeout_err_total = timeout_err.load(Ordering::Relaxed);
    let err_total = http_err_total + transport_err_total;
    let rps = if elapsed.as_secs_f64() > 0.0 {
        ok_total as f64 / elapsed.as_secs_f64()
    } else {
        0.0
    };
    Ok(LoadOutcome {
        ok: ok_total,
        errors: err_total,
        http_errors: http_err_total,
        transport_errors: transport_err_total,
        timeout_errors: timeout_err_total,
        elapsed,
        p50,
        p95,
        p99,
        rps,
    })
}
