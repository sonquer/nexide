//! CommonJS substrate for the nexide isolate.
//!
//! Components:
//! - [`BuiltinRegistry`] - thread-safe collection of `node:*` modules.
//! - [`CjsResolver`] / [`FsResolver`] - specifier resolution.

#![allow(clippy::doc_markdown)]

mod builtins;
mod errors;
mod registry;
mod resolver;

pub use builtins::{default_registry, register_node_builtins};
pub use errors::CjsError;
pub use registry::{BuiltinModule, BuiltinRegistry};
pub use resolver::{CjsResolver, FsResolver, ROOT_PARENT, Resolved};
