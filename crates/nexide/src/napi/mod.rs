//! N-API runtime: loads Node.js native addons (`*.node`) compiled
//! against ABI v9.

#![allow(unsafe_code, missing_docs)]

pub mod async_work;
pub mod bindings;
pub mod callbacks;
pub mod env;
pub mod loader;
pub mod references;
pub mod threadsafe;
pub mod types;

pub use env::{NapiContext, NapiEnv};
pub use loader::{NapiLoadError, load_native_module};
pub use types::{NapiStatus, NapiValueType, napi_env, napi_value};
