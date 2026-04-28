//! Spawn and supervise the two benchmark targets.
//!
//! The targets are the `nexide` release binary and `node` running the
//! same Next.js standalone bundle, so the comparison is apples-to-apples.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::time::{Instant, sleep};

/// Identifies which runtime hosts the Next.js bundle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TargetKind {
    /// The `nexide` release binary built from this workspace.
    Nexide,
    /// `node` running `e2e/next-fixture/.next/standalone/server.js`.
    Node,
    /// `deno run` against the same standalone `server.js` (Deno's
    /// Node-compatibility mode).
    Deno,
}

impl TargetKind {
    /// Stable lowercase label used in CLI flags and report rows.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Nexide => "nexide",
            Self::Node => "node",
            Self::Deno => "deno",
        }
    }
}

/// Owned handle to a running benchmark target. The underlying process
/// is killed and reaped on drop.
pub struct TargetHandle {
    child: Option<Child>,
    /// Address the target is bound to.
    pub addr: SocketAddr,
    /// Which runtime is running.
    pub kind: TargetKind,
}

impl TargetHandle {
    /// PID of the supervised process.
    ///
    /// # Errors
    /// Returns an error when the OS reports no PID for the child.
    pub fn pid(&self) -> Result<u32> {
        self.child
            .as_ref()
            .and_then(tokio::process::Child::id)
            .context("child has no pid")
    }

    /// Stop the underlying process and await its exit.
    ///
    /// # Errors
    /// Returns an error when the kill or wait syscalls fail.
    pub async fn shutdown(&mut self) -> Result<()> {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        Ok(())
    }
}

impl Drop for TargetHandle {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.start_kill();
        }
    }
}

async fn drain<R>(target: &'static str, label: &'static str, stream: R)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    let mut lines = BufReader::new(stream).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        tracing::debug!(target: "nexide_bench", source = target, runtime = label, "{line}");
    }
}

async fn wait_ready(addr: SocketAddr, timeout: Duration) -> Result<()> {
    let url = format!("http://{addr}/api/ping");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(500))
        .build()?;
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(resp) = client.get(&url).send().await
            && resp.status().is_success()
        {
            return Ok(());
        }
        sleep(Duration::from_millis(150)).await;
    }
    bail!("target {addr} failed to become ready within {timeout:?}");
}

/// Spawn either nexide or node listening on `addr`.
///
/// `workspace_root` should point to the Cargo workspace root.
///
/// # Errors
/// Returns an error when the artifacts are missing, the process fails
/// to spawn, or the readiness probe times out.
///
/// # Panics
/// Panics if the standalone bundle path lacks a parent directory,
/// which would indicate a corrupt workspace layout.
pub async fn spawn_target(
    kind: TargetKind,
    addr: SocketAddr,
    workspace_root: &Path,
    ready_timeout: Duration,
) -> Result<TargetHandle> {
    let mut cmd = match kind {
        TargetKind::Nexide => {
            let bin = workspace_root.join("target/release/nexide");
            if !bin.is_file() {
                bail!("missing release binary: {}", bin.display());
            }
            let fixture_dir = workspace_root.join("e2e/next-fixture");
            let mut c = Command::new(bin);
            c.arg("start")
                .arg(&fixture_dir)
                .arg("--hostname")
                .arg(addr.ip().to_string())
                .arg("--port")
                .arg(addr.port().to_string())
                .current_dir(workspace_root);
            c
        }
        TargetKind::Node => {
            let server_js: PathBuf =
                workspace_root.join("e2e/next-fixture/.next/standalone/server.js");
            if !server_js.is_file() {
                bail!("missing standalone bundle: {}", server_js.display());
            }
            let mut c = Command::new("node");
            c.arg(&server_js)
                .env("PORT", addr.port().to_string())
                .env("HOSTNAME", addr.ip().to_string())
                .current_dir(server_js.parent().unwrap());
            c
        }
        TargetKind::Deno => {
            let server_js: PathBuf =
                workspace_root.join("e2e/next-fixture/.next/standalone/server.js");
            if !server_js.is_file() {
                bail!("missing standalone bundle: {}", server_js.display());
            }
            let mut c = Command::new("deno");
            c.arg("run")
                .arg("--allow-all")
                .arg("--unstable-detect-cjs")
                .arg("--unstable-bare-node-builtins")
                .arg("--unstable-sloppy-imports")
                .arg("--quiet")
                .arg(&server_js)
                .env("PORT", addr.port().to_string())
                .env("HOSTNAME", addr.ip().to_string())
                .env("NODE_ENV", "production")
                .current_dir(server_js.parent().unwrap());
            c
        }
    };
    cmd.stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd.spawn().context("spawn target")?;
    if let Some(out) = child.stdout.take() {
        tokio::spawn(drain("stdout", kind.label(), out));
    }
    if let Some(err) = child.stderr.take() {
        tokio::spawn(drain("stderr", kind.label(), err));
    }
    wait_ready(addr, ready_timeout).await?;
    Ok(TargetHandle {
        child: Some(child),
        addr,
        kind,
    })
}
