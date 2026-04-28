//! Native `/_next/image` HTTP-level optimizer.
//!
//! Bypasses V8 entirely: decodes, resizes (SIMD via `fast_image_resize`),
//! and re-encodes images directly in Rust. Result is on-disk cached so
//! repeat requests are zero-CPU streams. Replaces Next's `sharp`-backed
//! pipeline for the `/_next/image` route.

mod cache;
mod config;
mod glob;
mod handler;
mod memory;
mod pipeline;

pub use config::ImageConfig;
pub use handler::next_image_service;
