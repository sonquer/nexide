//! Failure modes of the dispatcher layer.

use thiserror::Error;

use crate::engine::EngineError;
use crate::ops::{RequestMetaError, ResponseError};

/// Errors that can occur while delivering an HTTP request to the
/// JavaScript handler and harvesting the response.
#[derive(Debug, Error)]
pub enum DispatchError {
    /// The request line failed validation before the JS layer could
    /// see it.
    #[error("dispatch: invalid request meta: {0}")]
    BadRequest(#[source] RequestMetaError),

    /// The request body could not be read into memory.
    #[error("dispatch: failed to read request body: {0}")]
    BodyRead(String),

    /// The dedicated isolate worker has shut down.
    #[error("dispatch: worker thread is no longer accepting work")]
    WorkerGone,

    /// The handler did not produce a response.
    #[error("dispatch: handler returned without finishing the response")]
    NoResponse,

    /// The response slot was malformed.
    #[error("dispatch: response slot error: {0}")]
    Response(#[source] ResponseError),

    /// Boot or runtime failure inside the engine.
    #[error("dispatch: engine error: {0}")]
    Engine(#[source] EngineError),
}
