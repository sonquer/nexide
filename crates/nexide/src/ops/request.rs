//! HTTP request slot exposed to JavaScript via op-calls.
//!
//! A [`RequestSlot`] is the V8-side representation of an in-flight HTTP
//! request: method, URI, headers and a [`bytes::Bytes`] body buffer
//! that is shared zero-copy with the JS side via
//! [`op_nexide_read_body`](super::extension::op_nexide_read_body). The
//! slot is taken out of the engine's `OpState` for the duration of one
//! request; concurrency comes from running multiple isolates, never
//! from sharing a slot across threads.

use bytes::Bytes;
use thiserror::Error;

/// Subset of the request line surfaced to JavaScript.
///
/// Method and URI are kept as already-validated `String`s (the
/// constructor enforces UTF-8 and length bounds). This keeps the JS
/// boundary on a Fast API eligible path —
/// no allocation per call once the slot is staged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestMeta {
    method: String,
    uri: String,
}

/// Hard cap for both method and URI. Larger inputs are rejected with
/// [`RequestMetaError::TooLong`] so that an attacker cannot make us
/// allocate unbounded JS strings.
pub const REQUEST_META_MAX_LEN: usize = 8 * 1024;

impl RequestMeta {
    /// Constructs a [`RequestMeta`] from raw method + URI strings.
    ///
    /// # Errors
    ///
    /// * [`RequestMetaError::EmptyMethod`] / [`RequestMetaError::EmptyUri`]
    ///   — either component is the empty string.
    /// * [`RequestMetaError::TooLong`] — either component exceeds
    ///   [`REQUEST_META_MAX_LEN`] bytes.
    /// * [`RequestMetaError::InvalidMethod`] — the method contains a
    ///   character outside the HTTP token set defined by RFC 7230.
    pub fn try_new(method: impl Into<String>, uri: impl Into<String>) -> Result<Self, RequestMetaError> {
        let method = method.into();
        let uri = uri.into();
        if method.is_empty() {
            return Err(RequestMetaError::EmptyMethod);
        }
        if uri.is_empty() {
            return Err(RequestMetaError::EmptyUri);
        }
        if method.len() > REQUEST_META_MAX_LEN {
            return Err(RequestMetaError::TooLong { field: "method" });
        }
        if uri.len() > REQUEST_META_MAX_LEN {
            return Err(RequestMetaError::TooLong { field: "uri" });
        }
        if !method.bytes().all(is_http_token_byte) {
            return Err(RequestMetaError::InvalidMethod);
        }
        Ok(Self { method, uri })
    }

    /// Returns the HTTP method (e.g. `GET`, `POST`).
    #[must_use]
    pub fn method(&self) -> &str {
        &self.method
    }

    /// Returns the request-target as it appeared on the wire.
    #[must_use]
    pub fn uri(&self) -> &str {
        &self.uri
    }
}

/// RFC 7230 "tchar" classifier. We keep the table inline to avoid a
/// dependency on `httparse` for one byte test.
const fn is_http_token_byte(b: u8) -> bool {
    matches!(
        b,
        b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+'
        | b'-' | b'.' | b'^' | b'_' | b'`' | b'|' | b'~'
        | b'0'..=b'9'
        | b'A'..=b'Z'
        | b'a'..=b'z'
    )
}

/// Failure modes when constructing a [`RequestMeta`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum RequestMetaError {
    /// Method string was empty.
    #[error("request meta: method must not be empty")]
    EmptyMethod,

    /// URI string was empty.
    #[error("request meta: uri must not be empty")]
    EmptyUri,

    /// Either component exceeded [`REQUEST_META_MAX_LEN`].
    #[error("request meta: {field} exceeds {limit} bytes", limit = REQUEST_META_MAX_LEN)]
    TooLong {
        /// Which component triggered the limit (`"method"` or `"uri"`).
        field: &'static str,
    },

    /// Method contains a byte outside the RFC 7230 token set.
    #[error("request meta: method contains an invalid token character")]
    InvalidMethod,
}

/// Header pair as exposed to the JS layer.
///
/// Names are stored lowercased so that JS receives a canonical form
/// regardless of how the upstream client capitalised them. Values are
/// kept as-is — the HTTP spec mandates ASCII for headers but does not
/// require any specific case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderPair {
    /// Lowercased header name (e.g. `content-type`).
    pub name: String,
    /// Header value, verbatim from the wire.
    pub value: String,
}

/// Per-request state held inside the engine's `OpState` while the
/// JavaScript handler runs.
///
/// The struct is intentionally not `Clone` — there is at most one slot
/// per isolate at a time, mirroring the single-request semantics of an
/// HTTP exchange.
#[derive(Debug)]
pub struct RequestSlot {
    meta: RequestMeta,
    headers: Vec<HeaderPair>,
    body: Bytes,
    body_cursor: usize,
}

impl RequestSlot {
    /// Wraps validated request data in a slot ready to be planted in
    /// `OpState`.
    #[must_use]
    pub const fn new(meta: RequestMeta, headers: Vec<HeaderPair>, body: Bytes) -> Self {
        Self {
            meta,
            headers,
            body,
            body_cursor: 0,
        }
    }
}

/// JS-facing read interface for [`RequestSlot`].
///
/// Implemented by [`RequestSlot`] in production and by an in-memory
/// double in unit tests; the trait keeps the op layer independent of
/// the concrete storage (Dependency Inversion).
pub trait RequestSource {
    /// Returns the request line. Pure (Query).
    fn meta(&self) -> &RequestMeta;

    /// Returns the headers. Pure (Query).
    fn headers(&self) -> &[HeaderPair];

    /// Streams the next body fragment into `dst`.
    ///
    /// Returns the number of bytes written (`0` once the body is
    /// drained). The cursor is advanced — calling `read_body` is a
    /// Command in CQS terms.
    fn read_body(&mut self, dst: &mut [u8]) -> usize;
}

impl RequestSource for RequestSlot {
    fn meta(&self) -> &RequestMeta {
        &self.meta
    }

    fn headers(&self) -> &[HeaderPair] {
        &self.headers
    }

    fn read_body(&mut self, dst: &mut [u8]) -> usize {
        let remaining = &self.body[self.body_cursor..];
        let n = remaining.len().min(dst.len());
        dst[..n].copy_from_slice(&remaining[..n]);
        self.body_cursor += n;
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn try_new_accepts_canonical_method_and_uri() {
        let meta = RequestMeta::try_new("GET", "/").expect("happy path");
        assert_eq!(meta.method(), "GET");
        assert_eq!(meta.uri(), "/");
    }

    #[test]
    fn try_new_rejects_empty_method() {
        assert_eq!(
            RequestMeta::try_new("", "/").unwrap_err(),
            RequestMetaError::EmptyMethod
        );
    }

    #[test]
    fn try_new_rejects_empty_uri() {
        assert_eq!(
            RequestMeta::try_new("GET", "").unwrap_err(),
            RequestMetaError::EmptyUri
        );
    }

    #[test]
    fn try_new_rejects_method_with_space() {
        assert_eq!(
            RequestMeta::try_new("GE T", "/").unwrap_err(),
            RequestMetaError::InvalidMethod
        );
    }

    #[test]
    fn try_new_rejects_overlong_uri() {
        let huge = "/".repeat(REQUEST_META_MAX_LEN + 1);
        match RequestMeta::try_new("GET", huge).unwrap_err() {
            RequestMetaError::TooLong { field } => assert_eq!(field, "uri"),
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn read_body_drains_in_chunks_and_signals_end() {
        let body = Bytes::from_static(b"hello world");
        let mut slot = RequestSlot::new(
            RequestMeta::try_new("GET", "/").unwrap(),
            vec![],
            body,
        );

        let mut buf = [0u8; 5];
        assert_eq!(slot.read_body(&mut buf), 5);
        assert_eq!(&buf, b"hello");
        assert_eq!(slot.read_body(&mut buf), 5);
        assert_eq!(&buf, b" worl");
        assert_eq!(slot.read_body(&mut buf), 1);
        assert_eq!(&buf[..1], b"d");
        assert_eq!(slot.read_body(&mut buf), 0);
    }

    #[test]
    fn header_order_is_preserved_for_multi_value() {
        let headers = vec![
            HeaderPair { name: "set-cookie".to_owned(), value: "a=1".to_owned() },
            HeaderPair { name: "set-cookie".to_owned(), value: "b=2".to_owned() },
        ];
        let slot = RequestSlot::new(
            RequestMeta::try_new("GET", "/").unwrap(),
            headers,
            Bytes::new(),
        );
        assert_eq!(slot.headers().len(), 2);
        assert_eq!(slot.headers()[0].value, "a=1");
        assert_eq!(slot.headers()[1].value, "b=2");
    }
}
