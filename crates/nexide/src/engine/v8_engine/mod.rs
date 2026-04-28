//! Concrete V8-backed [`crate::engine::IsolateHandle`] implementation.
//!
//! The engine is intentionally small in surface - it exposes a single
//! `boot_with_polyfills` entrypoint, an `enqueue` helper that hands a
//! request off to the JS-side handler, and a `pump` that advances V8's
//! microtask queue. Lifecycle policy (recycle, rebuild) lives in the
//! pool; this module is purely the runtime <-> V8 boundary.
//!
//! Module layout:
//!
//! * [`bridge`]       - host state injected into the isolate slot.
//! * [`modules`]      - ESM module map + filesystem resolver.
//! * [`ops_bridge`]   - installs `globalThis.__nexide` and
//!   `globalThis.Nexide.core.ops` from V8 function templates.
//! * [`async_ops`]    - generic infrastructure that lets ops return
//!   promises resolved by off-isolate `tokio` work.
//! * [`handle_table`] - generic side table mapping integer ids to
//!   per-isolate Rust resources (TCP sockets, child processes, …).
//! * [`bootstrap`]    - runs the polyfill bootstrap script.
//! * [`engine`]       - [`V8Engine`] struct and `IsolateHandle` impl.

mod async_ops;
mod bootstrap;
mod bridge;
mod engine;
mod handle_table;
mod modules;
mod ops_bridge;

pub use engine::{BootContext, V8Engine};
