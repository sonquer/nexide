//! V8 engine layer.
//!
//! Built directly on top of `rusty_v8`. Higher layers (worker pool,
//! HTTP shield) depend only on the [`IsolateHandle`] trait — never on
//! the concrete [`V8Engine`] — keeping them testable without pulling
//! V8 into the test build.

mod errors;
mod heap_config;
mod isolate;
mod v8_engine;

pub mod cjs;

pub use errors::EngineError;
pub use heap_config::{HeapLimitConfig, heap_limit_from_env};
pub use isolate::{HeapStats, IsolateHandle};
pub use v8_engine::{BootContext, V8Engine};
