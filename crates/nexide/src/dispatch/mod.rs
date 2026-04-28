//! Cross-thread bridge between the Axum HTTP shield (multi-threaded
//! Tokio) and the `!Send` [`crate::engine::V8Engine`] running on a
//! dedicated worker thread.
//!
//! The trait [`EngineDispatcher`] is the DIP boundary used by
//! [`crate::server::NextBridgeHandler`]; the production implementation
//! [`IsolateDispatcher`] owns one isolate, while tests can substitute
//! lightweight in-memory doubles.

mod dispatcher;
mod errors;

pub use dispatcher::{EngineDispatcher, IsolateDispatcher, ProtoRequest};
pub use errors::DispatchError;
