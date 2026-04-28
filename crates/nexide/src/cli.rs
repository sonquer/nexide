//! Command-line interface for the `nexide` binary.
//!
//! Subcommands mirror Next.js' own CLI surface so that a project that
//! already uses `next start` / `next dev` / `next build` can adopt
//! `nexide` as a drop-in replacement on the production path.
//!
//!  * [`Command::Start`] boots the native Rust runtime against the
//!    `next build` standalone output. It is the primary value-add of
//!    this crate — Axum + V8 with no Node.js process in the loop.
//!  * [`Command::Dev`] and [`Command::Build`] are honest passthroughs
//!    to Next.js' own CLI: development requires Turbopack/SWC and HMR,
//!    which are out of scope for a Rust runtime, and `build` is a
//!    JS-only step. Routing them through `nexide` keeps the CLI
//!    coherent so users do not need to memorise two binaries.
//!
//! The argument types are pure value objects (data-only structs).
//! Behaviour lives in [`crate::lib::run`] — keeps parsing and
//! execution separable for testing (CQS).
use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// Top-level CLI parser.
#[derive(Debug, Parser)]
#[command(
    name = "nexide",
    version,
    about = "Native Next.js runtime in Rust",
    propagate_version = true
)]
pub struct Cli {
    /// Selected subcommand.
    #[command(subcommand)]
    pub command: Command,
}

/// One subcommand per supported lifecycle stage.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run a built Next.js app in production mode using the native
    /// Rust runtime.
    ///
    /// Requires that `next build` has already been executed with
    /// `output: 'standalone'` enabled in `next.config.js`, producing
    /// `<dir>/.next/standalone/server.js`.
    Start(StartArgs),

    /// Run the app in development mode by delegating to `next dev`.
    ///
    /// The Rust runtime intentionally does not host a JavaScript
    /// bundler — Turbopack/SWC and HMR are reused from the user's
    /// installed Next.js.
    Dev(DevArgs),

    /// Build the app by delegating to `next build`.
    Build(BuildArgs),
}

/// Arguments for [`Command::Start`].
#[derive(Debug, Parser)]
pub struct StartArgs {
    /// Path to the Next.js project root. Defaults to the current
    /// working directory.
    #[arg(default_value = ".")]
    pub dir: PathBuf,

    /// TCP port to bind to. Falls back to the `PORT` environment
    /// variable, then `3000` (matching `next start`).
    #[arg(short = 'p', long, env = "PORT", default_value_t = 3000)]
    pub port: u16,

    /// Hostname or IP address to bind to. Falls back to the
    /// `HOSTNAME` environment variable, then `127.0.0.1`.
    #[arg(short = 'H', long, env = "HOSTNAME", default_value = "127.0.0.1")]
    pub hostname: String,
}

/// Arguments for [`Command::Dev`].
#[derive(Debug, Parser)]
pub struct DevArgs {
    /// Path to the Next.js project root. Defaults to the current
    /// working directory.
    #[arg(default_value = ".")]
    pub dir: PathBuf,

    /// TCP port forwarded to `next dev` via `--port`.
    #[arg(short = 'p', long, env = "PORT", default_value_t = 3000)]
    pub port: u16,

    /// Hostname forwarded to `next dev` via `--hostname`.
    #[arg(short = 'H', long, env = "HOSTNAME", default_value = "127.0.0.1")]
    pub hostname: String,

    /// Enable Turbopack (`next dev --turbo`).
    #[arg(long)]
    pub turbo: bool,
}

/// Arguments for [`Command::Build`].
#[derive(Debug, Parser)]
pub struct BuildArgs {
    /// Path to the Next.js project root. Defaults to the current
    /// working directory.
    #[arg(default_value = ".")]
    pub dir: PathBuf,
}
