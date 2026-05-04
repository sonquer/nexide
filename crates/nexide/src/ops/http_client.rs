//! Host-side HTTP/HTTPS client backing `node:https` (and the
//! outbound subset of `node:http`).
//!
//! `request` accepts the materialised view of a Node options bag -
//! method, URL, headers, body - and returns a `ResponseHandle`
//! describing the status line, headers, and the streaming body.
//! The body is delivered chunk-by-chunk through a Tokio `mpsc`
//! channel so JavaScript can consume large responses without
//! buffering the whole payload in memory.
//!
//! All TLS verification flows through the rustls trust store
//! configured in [`super::tls`]. Plain HTTP works on the same code
//! path because reqwest auto-selects the scheme from the URL.
//!
//! `tracing` records emit on `nexide::ops::http`. Request lifecycle
//! (dispatch, response headers received, body completed) logs at
//! `debug`; per-chunk delivery at `trace`; transport / decode
//! failures at `warn`.

use std::sync::OnceLock;
use std::time::Duration;

use reqwest::Client;
use tokio::sync::mpsc;

use super::net::NetError;

const LOG_TARGET: &str = "nexide::ops::http";

fn shared_client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        Client::builder()
            .use_rustls_tls()
            .pool_idle_timeout(Some(Duration::from_secs(30)))
            .build()
            .expect("reqwest client")
    })
}

/// Header pair `(name, value)`. Names are stored as-received so the
/// JS side can preserve canonical casing if it cares.
#[derive(Debug, Clone)]
pub struct HttpHeader {
    /// Header name, e.g. `"Content-Type"`.
    pub name: String,
    /// Header value, e.g. `"application/json"`.
    pub value: String,
}

/// Streaming response handle returned by [`request`].
///
/// The `body` channel yields each network chunk as it arrives. The
/// channel is closed once the response is fully consumed; the
/// receiver observes that as `recv() == None`.
pub struct ResponseHandle {
    /// HTTP status code (`200`, `404`, …).
    pub status: u16,
    /// Reason phrase (`"OK"`, `"Not Found"`, …). Empty for HTTP/2.
    pub status_text: String,
    /// Response headers, preserving order of arrival.
    pub headers: Vec<HttpHeader>,
    /// Body chunks streamed in arrival order. `Err(NetError)` is
    /// sent if a transport error happens mid-body.
    pub body: mpsc::UnboundedReceiver<Result<Vec<u8>, NetError>>,
}

/// Fully-buffered request payload. Streaming uploads are not yet
/// supported; passing very large bodies through this op will spike
/// memory usage. The Node-shaped streaming write loop on top of
/// [`super::net`] / [`super::tls`] is the right tool for that case.
pub struct HttpRequest {
    /// Method name, uppercased (`"GET"`, `"POST"`, …).
    pub method: String,
    /// Absolute URL including scheme.
    pub url: String,
    /// Request headers in declaration order.
    pub headers: Vec<HttpHeader>,
    /// Request body bytes; pass an empty `Vec` for body-less methods.
    pub body: Vec<u8>,
}

/// Issues `request` against `shared_client` and starts streaming
/// the response body.
///
/// # Errors
/// Returns `NetError` when URL parsing, header construction, or
/// the request itself fail. Body chunk errors are surfaced through
/// the `body` channel rather than returned synchronously.
#[tracing::instrument(
    target = "nexide::ops::http",
    level = "debug",
    name = "http_request",
    skip_all,
    fields(method = %req.method, url = %req.url, body_bytes = req.body.len()),
    err(level = "warn", Display),
)]
pub async fn request(req: HttpRequest) -> Result<ResponseHandle, NetError> {
    use reqwest::Method;
    use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

    let method = Method::from_bytes(req.method.as_bytes()).map_err(|_| {
        NetError::new(
            "ERR_INVALID_HTTP_METHOD",
            format!("bad method {}", req.method),
        )
    })?;

    let mut headers = HeaderMap::with_capacity(req.headers.len());
    for h in &req.headers {
        let name = HeaderName::from_bytes(h.name.as_bytes()).map_err(|_| {
            NetError::new(
                "ERR_INVALID_HEADER_NAME",
                format!("bad header name {}", h.name),
            )
        })?;
        let value = HeaderValue::from_str(&h.value).map_err(|_| {
            NetError::new(
                "ERR_INVALID_HEADER_VALUE",
                format!("bad header value for {}", h.name),
            )
        })?;
        headers.append(name, value);
    }

    let url_for_body = req.url.clone();
    let mut builder = shared_client().request(method, &req.url).headers(headers);
    if !req.body.is_empty() {
        builder = builder.body(req.body);
    }

    tracing::trace!(target: LOG_TARGET, "request dispatched to transport");

    let response = builder.send().await.map_err(|e| {
        let code = reqwest_error_code(&e);
        NetError::new(code, e.to_string())
    })?;

    let status = response.status().as_u16();
    let status_text = response
        .status()
        .canonical_reason()
        .unwrap_or("")
        .to_owned();
    let mut response_headers = Vec::with_capacity(response.headers().len());
    for (name, value) in response.headers().iter() {
        if let Ok(value_str) = value.to_str() {
            response_headers.push(HttpHeader {
                name: name.as_str().to_owned(),
                value: value_str.to_owned(),
            });
        }
    }

    tracing::debug!(
        target: LOG_TARGET,
        status,
        headers = response_headers.len(),
        "response head received",
    );

    let (tx, rx) = mpsc::unbounded_channel();
    tokio::task::spawn_local(stream_body(response, tx, url_for_body));

    Ok(ResponseHandle {
        status,
        status_text,
        headers: response_headers,
        body: rx,
    })
}

async fn stream_body(
    mut response: reqwest::Response,
    tx: mpsc::UnboundedSender<Result<Vec<u8>, NetError>>,
    url: String,
) {
    let trace_enabled = tracing::enabled!(target: LOG_TARGET, tracing::Level::TRACE);
    let mut chunks: u32 = 0;
    let mut bytes: u64 = 0;
    loop {
        match response.chunk().await {
            Ok(Some(buf)) => {
                if trace_enabled {
                    chunks = chunks.saturating_add(1);
                    bytes = bytes.saturating_add(buf.len() as u64);
                    tracing::trace!(
                        target: LOG_TARGET,
                        chunk_bytes = buf.len(),
                        chunk_index = chunks,
                        "response body chunk",
                    );
                }
                if tx.send(Ok(buf.to_vec())).is_err() {
                    tracing::debug!(
                        target: LOG_TARGET,
                        url = %url,
                        chunks,
                        bytes,
                        "response body receiver dropped; aborting stream",
                    );
                    return;
                }
            }
            Ok(None) => {
                tracing::debug!(
                    target: LOG_TARGET,
                    url = %url,
                    chunks,
                    bytes,
                    "response body stream completed",
                );
                return;
            }
            Err(err) => {
                let code = reqwest_error_code(&err);
                tracing::warn!(
                    target: LOG_TARGET,
                    url = %url,
                    code,
                    error = %err,
                    chunks,
                    bytes,
                    "response body transport error",
                );
                let _ = tx.send(Err(NetError::new(code, err.to_string())));
                return;
            }
        }
    }
}

fn reqwest_error_code(err: &reqwest::Error) -> &'static str {
    if err.is_timeout() {
        return "ETIMEDOUT";
    }
    if err.is_connect() {
        return "ECONNREFUSED";
    }
    if err.is_request() {
        return "ERR_INVALID_URL";
    }
    if err.is_decode() {
        return "ERR_DECODE";
    }
    if err.is_redirect() {
        return "ERR_TOO_MANY_REDIRECTS";
    }
    "EIO"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn invalid_method_rejects() {
        let req = HttpRequest {
            method: "GET\u{0}".to_owned(),
            url: "http://127.0.0.1/".to_owned(),
            headers: vec![],
            body: vec![],
        };
        let local = tokio::task::LocalSet::new();
        let result = local.run_until(async move { request(req).await }).await;
        assert!(result.is_err());
    }

    #[test]
    fn shared_client_is_cached() {
        let a = shared_client() as *const Client;
        let b = shared_client() as *const Client;
        assert_eq!(a, b);
    }
}
