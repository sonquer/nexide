//! Response side of the JS ↔ Rust op bridge.
//!
//! JavaScript builds a response by calling `__nexide.sendHead`,
//! `__nexide.sendChunk` and `__nexide.sendEnd`. Each call drives the
//! [`ResponseSink`] trait - the abstraction that the op layer talks to,
//! decoupling it from the concrete buffer (production: a `Vec<Bytes>`
//! plus a `oneshot` responder; tests: an in-memory recorder).

use bytes::Bytes;
use thiserror::Error;

/// Aggregated response built up by JS.
///
/// The struct is constructed incrementally: [`ResponseSlot::send_head`]
/// fixes status+headers, [`ResponseSlot::send_chunk`] appends body
/// bytes, [`ResponseSlot::finish`] takes ownership and yields the final
/// [`ResponsePayload`]. The state machine is enforced statically by
/// the trait method signatures.
#[derive(Debug, Default)]
pub struct ResponseSlot {
    head: Option<ResponseHead>,
    chunks: Vec<Bytes>,
    finished: bool,
}

/// Status + headers half of a response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseHead {
    /// HTTP status code (e.g. `200`, `404`).
    pub status: u16,
    /// Response headers in JS-supplied order.
    pub headers: Vec<(String, String)>,
}

/// Final response value handed back to the HTTP shield.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponsePayload {
    /// Status line and headers.
    pub head: ResponseHead,
    /// Concatenated body bytes (the op layer accepts arbitrary chunk
    /// sizes; aggregation happens here so the shield sees one buffer).
    pub body: Bytes,
}

/// Errors observed while assembling a response.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ResponseError {
    /// `send_head` was invoked twice on the same slot.
    #[error("response: head already sent")]
    HeadAlreadySent,

    /// `send_chunk` / `send_head` was invoked after `finish`.
    #[error("response: slot has been finalised")]
    AlreadyFinished,

    /// `send_chunk` was invoked before `send_head`.
    #[error("response: chunk sent before head")]
    ChunkBeforeHead,

    /// `finish` was invoked before `send_head`.
    #[error("response: finished without head")]
    FinishWithoutHead,

    /// Status code outside the RFC 9110 valid range.
    #[error("response: invalid status code {0}")]
    InvalidStatus(u16),
}

/// Behavioural contract used by the op layer to write a response.
///
/// `finish` consumes `self` so misuse-after-finish is caught at compile
/// time (strict CQS / type-state).
pub trait ResponseSink: Sized {
    /// Records status code and headers (Command).
    ///
    /// # Errors
    ///
    /// See [`ResponseError`] - the relevant variants are
    /// [`ResponseError::HeadAlreadySent`],
    /// [`ResponseError::AlreadyFinished`] and
    /// [`ResponseError::InvalidStatus`].
    fn send_head(&mut self, head: ResponseHead) -> Result<(), ResponseError>;

    /// Appends body bytes (Command).
    ///
    /// # Errors
    ///
    /// [`ResponseError::ChunkBeforeHead`] when invoked before
    /// [`ResponseSink::send_head`]; [`ResponseError::AlreadyFinished`]
    /// after [`ResponseSink::finish`].
    fn send_chunk(&mut self, chunk: Bytes) -> Result<(), ResponseError>;

    /// Consumes the sink and returns the assembled response.
    ///
    /// # Errors
    ///
    /// [`ResponseError::FinishWithoutHead`] when no head was sent.
    fn finish(self) -> Result<ResponsePayload, ResponseError>;
}

impl ResponseSlot {
    /// Creates an empty slot. Equivalent to `Self::default()`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl ResponseSink for ResponseSlot {
    fn send_head(&mut self, head: ResponseHead) -> Result<(), ResponseError> {
        if self.finished {
            return Err(ResponseError::AlreadyFinished);
        }
        if self.head.is_some() {
            return Err(ResponseError::HeadAlreadySent);
        }
        if !(100..=599).contains(&head.status) {
            return Err(ResponseError::InvalidStatus(head.status));
        }
        self.head = Some(head);
        Ok(())
    }

    fn send_chunk(&mut self, chunk: Bytes) -> Result<(), ResponseError> {
        if self.finished {
            return Err(ResponseError::AlreadyFinished);
        }
        if self.head.is_none() {
            return Err(ResponseError::ChunkBeforeHead);
        }
        if !chunk.is_empty() {
            self.chunks.push(chunk);
        }
        Ok(())
    }

    fn finish(mut self) -> Result<ResponsePayload, ResponseError> {
        let head = self.head.take().ok_or(ResponseError::FinishWithoutHead)?;
        self.finished = true;
        let body = match self.chunks.len() {
            0 => Bytes::new(),
            1 => self
                .chunks
                .pop()
                .expect("len==1 guarantees a chunk is present"),
            _ => {
                let total: usize = self.chunks.iter().map(bytes::Bytes::len).sum();
                let mut body = bytes::BytesMut::with_capacity(total);
                for chunk in &self.chunks {
                    body.extend_from_slice(chunk);
                }
                body.freeze()
            }
        };
        Ok(ResponsePayload { head, body })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok_head() -> ResponseHead {
        ResponseHead {
            status: 200,
            headers: vec![("content-type".to_owned(), "text/plain".to_owned())],
        }
    }

    #[test]
    fn happy_path_assembles_full_response() {
        let mut slot = ResponseSlot::new();
        slot.send_head(ok_head()).unwrap();
        slot.send_chunk(Bytes::from_static(b"po")).unwrap();
        slot.send_chunk(Bytes::from_static(b"ng")).unwrap();
        let payload = slot.finish().unwrap();
        assert_eq!(payload.head.status, 200);
        assert_eq!(&payload.body[..], b"pong");
    }

    #[test]
    fn rejects_double_head() {
        let mut slot = ResponseSlot::new();
        slot.send_head(ok_head()).unwrap();
        assert_eq!(
            slot.send_head(ok_head()).unwrap_err(),
            ResponseError::HeadAlreadySent
        );
    }

    #[test]
    fn rejects_chunk_before_head() {
        let mut slot = ResponseSlot::new();
        assert_eq!(
            slot.send_chunk(Bytes::from_static(b"x")).unwrap_err(),
            ResponseError::ChunkBeforeHead
        );
    }

    #[test]
    fn rejects_finish_without_head() {
        let slot = ResponseSlot::new();
        assert_eq!(slot.finish().unwrap_err(), ResponseError::FinishWithoutHead);
    }

    #[test]
    fn rejects_status_out_of_range() {
        let mut slot = ResponseSlot::new();
        let mut head = ok_head();
        head.status = 42;
        assert_eq!(
            slot.send_head(head).unwrap_err(),
            ResponseError::InvalidStatus(42)
        );
    }

    #[test]
    fn empty_chunks_are_skipped() {
        let mut slot = ResponseSlot::new();
        slot.send_head(ok_head()).unwrap();
        slot.send_chunk(Bytes::new()).unwrap();
        slot.send_chunk(Bytes::from_static(b"x")).unwrap();
        let payload = slot.finish().unwrap();
        assert_eq!(&payload.body[..], b"x");
    }

    #[test]
    fn single_chunk_passthrough_preserves_bytes_identity() {
        let mut slot = ResponseSlot::new();
        slot.send_head(ok_head()).unwrap();
        let payload_bytes = Bytes::from_static(b"single");
        slot.send_chunk(payload_bytes.clone()).unwrap();
        let payload = slot.finish().unwrap();
        assert_eq!(&payload.body[..], b"single");
        assert_eq!(payload.body.as_ptr(), payload_bytes.as_ptr());
    }

    #[test]
    fn no_chunks_yields_empty_body() {
        let mut slot = ResponseSlot::new();
        slot.send_head(ok_head()).unwrap();
        let payload = slot.finish().unwrap();
        assert!(payload.body.is_empty());
    }
}
