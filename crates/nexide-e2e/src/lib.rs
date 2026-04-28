//! End-to-end harness for booting the `nexide` release binary against
//! a real Next.js standalone bundle.
//!
//! The harness intentionally lives in its own crate so integration of
//! the runtime against an external Node.js artifact is decoupled from
//! the unit/integration tests of the runtime library itself.

#![deny(missing_docs)]

use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::time::{Instant, sleep};

/// Returns the workspace root by walking up two directories from the
/// crate manifest (`crates/nexide-e2e` → workspace root).
///
/// # Panics
/// Panics if the crate is not located two directories below a
/// workspace root, which would indicate a corrupt layout.
#[must_use]
pub fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .expect("workspace root")
}

/// Path to the example Next.js project's standalone build output.
#[must_use]
pub fn example_standalone() -> PathBuf {
    workspace_root().join("e2e/next-fixture/.next/standalone/server.js")
}

/// Path to the Prisma+SQLite fixture's standalone build output.
#[must_use]
pub fn prisma_sqlite_standalone() -> PathBuf {
    workspace_root().join("e2e/prisma-sqlite/.next/standalone/server.js")
}

/// Path to the release `nexide` binary.
#[must_use]
pub fn nexide_binary() -> PathBuf {
    workspace_root().join("target/release/nexide")
}

/// Returns `true` when both the example bundle and the release binary
/// are present. Tests that rely on full boot should skip themselves
/// when this returns `false` rather than fail.
#[must_use]
pub fn prerequisites_present() -> bool {
    example_standalone().is_file() && nexide_binary().is_file()
}

/// Returns `true` when the Prisma+SQLite fixture has been built and
/// the release nexide binary is present.
#[must_use]
pub fn prisma_prerequisites_present() -> bool {
    prisma_sqlite_standalone().is_file() && nexide_binary().is_file()
}

/// Pick a free TCP port on the loopback interface. The port is
/// released immediately, so callers should bind quickly to avoid a
/// race with another process.
///
/// # Errors
/// Returns an error when the OS refuses to allocate any free port.
pub fn pick_free_port() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").context("bind ephemeral port")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

/// Owned handle to a running nexide process; the process is killed
/// and reaped on drop so tests don't leak background servers.
pub struct NexideProcess {
    child: Option<Child>,
    addr: SocketAddr,
}

impl NexideProcess {
    /// The address the server is bound to.
    #[must_use]
    pub const fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Boot a release `nexide` against the example standalone bundle
    /// and wait until `/api/ping` responds 200 (or `timeout` elapses).
    ///
    /// # Errors
    /// Returns an error when prerequisites are missing, when the
    /// process fails to spawn, or when the readiness probe times out.
    pub async fn spawn(timeout: Duration) -> Result<Self> {
        if !prerequisites_present() {
            bail!(
                "missing prerequisites: build example with `npm run build` and run `cargo build --release` first"
            );
        }
        Self::spawn_at(workspace_root(), timeout).await
    }

    /// Boot a release `nexide` from `cwd` (which must contain a
    /// `.next/standalone/server.js` resolvable via the production
    /// entrypoint resolver) and wait until `/api/ping` responds 200.
    ///
    /// # Errors
    /// Same conditions as [`Self::spawn`].
    pub async fn spawn_at(cwd: PathBuf, timeout: Duration) -> Result<Self> {
        if !nexide_binary().is_file() {
            bail!("missing release nexide binary; run `cargo build --release`");
        }
        let port = pick_free_port()?;
        let addr: SocketAddr = format!("127.0.0.1:{port}").parse()?;
        let mut cmd = Command::new(nexide_binary());
        cmd.arg("start")
            .arg(".")
            .arg("--hostname")
            .arg(addr.ip().to_string())
            .arg("--port")
            .arg(addr.port().to_string())
            .current_dir(cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = cmd.spawn().context("spawn nexide")?;
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(drain("nexide.stderr", stderr));
        }
        if let Some(stdout) = child.stdout.take() {
            tokio::spawn(drain("nexide.stdout", stdout));
        }
        wait_ready(addr, timeout).await?;
        Ok(Self {
            child: Some(child),
            addr,
        })
    }

    /// Stop the underlying process and wait for it to exit.
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

impl Drop for NexideProcess {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.start_kill();
        }
    }
}

async fn drain<R>(target: &'static str, stream: R)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    let mut lines = BufReader::new(stream).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        tracing::debug!(target: "nexide_e2e", source = target, "{line}");
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
        sleep(Duration::from_millis(100)).await;
    }
    bail!("nexide failed to become ready at {url} within {timeout:?}");
}
