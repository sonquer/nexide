//! Host-side `child_process` façade backing `node:child_process`.
//!
//! Wraps `tokio::process::Command` so spawned children expose
//! handle ids for stdin/stdout/stderr that the JS layer drives
//! through op-based reads and writes. The full shape of
//! [`ChildHandle`] gives the bridge enough to implement
//! `spawn`/`exec`/`execFile`; `spawnSync` / `execSync` are not
//! supported because blocking the V8 thread would stall the
//! event loop.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::io;
use std::process::Stdio;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};

use super::net::NetError;

/// Stdio routing requested by the JS caller.
#[derive(Debug, Clone, Copy)]
pub enum StdioMode {
    /// Pipe to the parent through a captured handle.
    Pipe,
    /// Inherit the parent's stream.
    Inherit,
    /// Discard (`/dev/null` equivalent on each platform).
    Ignore,
}

impl StdioMode {
    fn into_stdio(self) -> Stdio {
        match self {
            Self::Pipe => Stdio::piped(),
            Self::Inherit => Stdio::inherit(),
            Self::Ignore => Stdio::null(),
        }
    }
}

/// Materialised `spawn` request.
pub struct SpawnRequest {
    /// Command to run (executable name or absolute path).
    pub command: String,
    /// Argument list passed verbatim — the host does not reshell.
    pub args: Vec<String>,
    /// Optional working directory; falls back to the parent's CWD
    /// when `None`.
    pub cwd: Option<String>,
    /// Environment override. Empty map preserves the parent env.
    pub env: HashMap<String, String>,
    /// Whether the child inherits the parent env before applying
    /// `env`.
    pub clear_env: bool,
    /// Stdio routing for `stdin`, `stdout`, `stderr` (in order).
    pub stdio: [StdioMode; 3],
}

/// Successful spawn descriptor. Each `Option` is populated only when
/// the matching [`StdioMode`] was `Pipe`.
#[derive(Debug)]
pub struct ChildHandle {
    /// Operating-system process id.
    pub pid: u32,
    /// Tokio child handle, kept for `wait` / `kill`.
    pub child: Child,
    /// Captured stdin pipe.
    pub stdin: Option<ChildStdin>,
    /// Captured stdout pipe.
    pub stdout: Option<ChildStdout>,
    /// Captured stderr pipe.
    pub stderr: Option<ChildStderr>,
}

/// Spawns the requested process.
///
/// # Errors
/// Returns a [`NetError`] (re-using the Node-style error code mapping)
/// when the executable cannot be located or the OS refuses to spawn.
pub fn spawn(req: SpawnRequest) -> Result<ChildHandle, NetError> {
    let mut cmd = Command::new(OsStr::new(&req.command));
    cmd.args(req.args.iter().map(OsStr::new));
    if let Some(cwd) = req.cwd.as_ref() {
        cmd.current_dir(cwd);
    }
    if req.clear_env {
        cmd.env_clear();
    }
    for (k, v) in &req.env {
        cmd.env(k, v);
    }
    cmd.stdin(req.stdio[0].into_stdio());
    cmd.stdout(req.stdio[1].into_stdio());
    cmd.stderr(req.stdio[2].into_stdio());
    cmd.kill_on_drop(false);

    let mut child = cmd.spawn().map_err(map_io_err)?;
    let pid = child.id().unwrap_or(0);
    let stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    Ok(ChildHandle {
        pid,
        child,
        stdin,
        stdout,
        stderr,
    })
}

/// Reads up to `max` bytes from `pipe`. `Ok(None)` signals EOF.
///
/// # Errors
/// Forwards transport errors as `NetError`.
pub async fn read_pipe<R>(pipe: &mut R, max: usize) -> Result<Option<Vec<u8>>, NetError>
where
    R: AsyncReadExt + Unpin,
{
    let mut buf = vec![0u8; max.max(1)];
    let n = pipe.read(&mut buf).await.map_err(map_io_err)?;
    if n == 0 {
        return Ok(None);
    }
    buf.truncate(n);
    Ok(Some(buf))
}

/// Writes `data` to `pipe` flushing any partial writes.
///
/// # Errors
/// Forwards transport errors as `NetError`.
pub async fn write_pipe<W>(pipe: &mut W, data: &[u8]) -> Result<(), NetError>
where
    W: AsyncWriteExt + Unpin,
{
    pipe.write_all(data).await.map_err(map_io_err)?;
    pipe.flush().await.map_err(map_io_err)
}

/// Result of a successful `wait` call.
#[derive(Debug, Clone, Copy)]
pub struct ExitInfo {
    /// Exit code, when the process terminated normally.
    pub code: Option<i32>,
    /// POSIX signal number, when the process was terminated by a
    /// signal (Unix only).
    pub signal: Option<i32>,
}

/// Waits for the child to exit.
///
/// # Errors
/// Returns a `NetError` if the OS reports a wait failure.
pub async fn wait(child: &mut Child) -> Result<ExitInfo, NetError> {
    let status = child.wait().await.map_err(map_io_err)?;
    Ok(ExitInfo {
        code: status.code(),
        #[cfg(unix)]
        signal: std::os::unix::process::ExitStatusExt::signal(&status),
        #[cfg(not(unix))]
        signal: None,
    })
}

/// Sends a signal-equivalent kill to the child.
///
/// On Unix the platform-specific signal id is honoured; on other
/// platforms the kill always terminates the process regardless of
/// the requested signal.
///
/// # Errors
/// Returns a `NetError` if the OS rejects the kill request.
pub fn kill(child: &mut Child, signal: i32) -> Result<(), NetError> {
    #[cfg(unix)]
    {
        if let Some(pid) = child.id() {
            let pid = pid as i32;
            let res =
                unsafe { libc::kill(pid, signal) };
            if res != 0 {
                return Err(map_io_err(io::Error::last_os_error()));
            }
            return Ok(());
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = signal;
        child.start_kill().map_err(map_io_err)
    }
}

fn map_io_err(err: io::Error) -> NetError {
    let code = match err.kind() {
        io::ErrorKind::NotFound => "ENOENT",
        io::ErrorKind::PermissionDenied => "EACCES",
        io::ErrorKind::AlreadyExists => "EEXIST",
        io::ErrorKind::InvalidInput => "EINVAL",
        io::ErrorKind::Interrupted => "EINTR",
        _ => "EIO",
    };
    NetError::new(code, err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local_runtime<F: std::future::Future<Output = ()>>(fut: F) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let local = tokio::task::LocalSet::new();
        local.block_on(&rt, fut);
    }

    #[test]
    fn spawn_echo_returns_stdout() {
        local_runtime(async {
            let req = SpawnRequest {
                command: if cfg!(windows) { "cmd".into() } else { "/bin/echo".into() },
                args: if cfg!(windows) { vec!["/C".into(), "echo hi".into()] } else { vec!["hi".into()] },
                cwd: None,
                env: HashMap::new(),
                clear_env: false,
                stdio: [StdioMode::Ignore, StdioMode::Pipe, StdioMode::Ignore],
            };
            let mut handle = spawn(req).expect("spawn");
            let mut stdout = handle.stdout.take().expect("stdout");
            let mut buf = Vec::new();
            stdout.read_to_end(&mut buf).await.expect("read");
            let info = wait(&mut handle.child).await.expect("wait");
            assert_eq!(info.code, Some(0));
            let out = String::from_utf8_lossy(&buf);
            assert!(out.contains("hi"), "stdout was {out:?}");
        });
    }

    #[test]
    fn spawn_missing_program_yields_enoent() {
        let req = SpawnRequest {
            command: "/this/path/does/not/exist/program".into(),
            args: vec![],
            cwd: None,
            env: HashMap::new(),
            clear_env: false,
            stdio: [StdioMode::Ignore, StdioMode::Ignore, StdioMode::Ignore],
        };
        let err = spawn(req).expect_err("should error");
        assert_eq!(err.code, "ENOENT");
    }
}
