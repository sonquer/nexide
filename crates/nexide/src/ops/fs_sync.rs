//! Synchronous filesystem ops backing the `node:fs` polyfill.
//!
//! A trait pair (`FsBackend` for I/O, `Sandbox` for path admission)
//! keeps the op layer free of host-specific knowledge:
//!
//! - production: [`RealFs`] (delegates to `std::fs`) +
//!   [`PathSandbox`] (canonicalising root checker).
//! - tests: [`MemoryFs`] (an in-memory map) + the test-only
//!   `AlwaysAllow` sandbox.
//!
//! Every op is a single Command or Query - no piecemeal state. Errors
//! are reported as `(code, message)` tuples that JS reshapes into
//! Node-shaped `Error` objects (`err.code`, `err.message`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use serde::Serialize;

const LOG_TARGET: &str = "nexide::ops::fs";

fn log_err(op: &'static str, path: &str, err: &FsError) {
    tracing::trace!(
        target: LOG_TARGET,
        op,
        path,
        code = err.code,
        message = %err.message,
        "fs op error",
    );
}

/// Recoverable filesystem error converted to a Node-style code/message
/// pair on the JS boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsError {
    /// Node.js error code (`ENOENT`, `EACCES`, …).
    pub code: &'static str,
    /// Human-readable description suitable for `err.message`.
    pub message: String,
}

impl FsError {
    /// Builds a new error with the supplied `code`/`message`.
    #[must_use]
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    fn from_io(err: &std::io::Error, path: &Path) -> Self {
        use std::io::ErrorKind as K;
        let code: &'static str = match err.kind() {
            K::NotFound => "ENOENT",
            K::PermissionDenied => "EACCES",
            K::AlreadyExists => "EEXIST",
            K::InvalidInput | K::InvalidData => "EINVAL",
            K::Unsupported => "ENOSYS",
            _ => "EIO",
        };
        Self {
            code,
            message: format!("{code}: {err} ({})", path.display()),
        }
    }
}

/// Stat tuple returned to JS; mirrors Node `Stats` minimal subset.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct FsStat {
    /// Total file size in bytes.
    pub size: u64,
    /// `true` for regular files.
    pub is_file: bool,
    /// `true` for directories.
    pub is_dir: bool,
    /// `true` for symbolic links (only ever `true` from `lstat`).
    pub is_symlink: bool,
    /// Modification time in milliseconds since the Unix epoch.
    pub mtime_ms: f64,
    /// Mode bits - best-effort; `0` when not available.
    pub mode: u32,
}

/// Single directory entry projected for JS.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DirEntry {
    /// File or subdirectory name (no path component).
    pub name: String,
    /// `true` when the entry is a directory.
    pub is_dir: bool,
    /// `true` when the entry is a symbolic link.
    pub is_symlink: bool,
}

/// Filesystem back-end behind the op layer (DIP).
pub trait FsBackend: Send + Sync + 'static {
    /// Reads the entire contents of `path`.
    ///
    /// # Errors
    /// Propagates host I/O failures.
    fn read(&self, path: &Path) -> Result<Vec<u8>, FsError>;
    /// Writes `data` atomically replacing any prior contents at `path`.
    ///
    /// # Errors
    /// Propagates host I/O failures.
    fn write(&self, path: &Path, data: &[u8]) -> Result<(), FsError>;
    /// Returns metadata for `path`. When `follow` is `false` symbolic
    /// links are not dereferenced (Node `lstat` semantics).
    ///
    /// # Errors
    /// Propagates host I/O failures.
    fn stat(&self, path: &Path, follow: bool) -> Result<FsStat, FsError>;
    /// Returns `true` when `path` exists (any kind).
    fn exists(&self, path: &Path) -> bool;
    /// Lists immediate children of `path` (non-recursive).
    ///
    /// # Errors
    /// Propagates host I/O failures.
    fn read_dir(&self, path: &Path) -> Result<Vec<DirEntry>, FsError>;
    /// Creates a directory; when `recursive` is `true` mirrors Node
    /// `mkdir -p`.
    ///
    /// # Errors
    /// Propagates host I/O failures.
    fn mkdir(&self, path: &Path, recursive: bool) -> Result<(), FsError>;
    /// Removes a file or directory tree.
    ///
    /// # Errors
    /// Propagates host I/O failures.
    fn remove(&self, path: &Path, recursive: bool) -> Result<(), FsError>;
    /// Copies `from` over `to` (truncating any existing file).
    ///
    /// # Errors
    /// Propagates host I/O failures.
    fn copy(&self, from: &Path, to: &Path) -> Result<(), FsError>;
    /// Reads the target of a symbolic link.
    ///
    /// # Errors
    /// Propagates host I/O failures.
    fn read_link(&self, path: &Path) -> Result<PathBuf, FsError>;
    /// Returns the canonical path of `path`.
    ///
    /// # Errors
    /// Propagates host I/O failures.
    fn realpath(&self, path: &Path) -> Result<PathBuf, FsError>;
}

/// Standard-library backed [`FsBackend`].
#[derive(Debug, Default)]
pub struct RealFs;

fn ms_since_epoch(t: SystemTime) -> f64 {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

#[cfg(unix)]
fn mode_of(meta: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode()
}

#[cfg(not(unix))]
const fn mode_of(_meta: &std::fs::Metadata) -> u32 {
    0
}

impl FsBackend for RealFs {
    fn read(&self, path: &Path) -> Result<Vec<u8>, FsError> {
        std::fs::read(path).map_err(|e| FsError::from_io(&e, path))
    }
    fn write(&self, path: &Path, data: &[u8]) -> Result<(), FsError> {
        std::fs::write(path, data).map_err(|e| FsError::from_io(&e, path))
    }
    fn stat(&self, path: &Path, follow: bool) -> Result<FsStat, FsError> {
        let meta = if follow {
            std::fs::metadata(path)
        } else {
            std::fs::symlink_metadata(path)
        }
        .map_err(|e| FsError::from_io(&e, path))?;
        Ok(FsStat {
            size: meta.len(),
            is_file: meta.is_file(),
            is_dir: meta.is_dir(),
            is_symlink: meta.file_type().is_symlink(),
            mtime_ms: meta.modified().map(ms_since_epoch).unwrap_or(0.0),
            mode: mode_of(&meta),
        })
    }
    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }
    fn read_dir(&self, path: &Path) -> Result<Vec<DirEntry>, FsError> {
        let iter = std::fs::read_dir(path).map_err(|e| FsError::from_io(&e, path))?;
        let mut out = Vec::new();
        for entry in iter {
            let entry = entry.map_err(|e| FsError::from_io(&e, path))?;
            let ft = entry
                .file_type()
                .map_err(|e| FsError::from_io(&e, &entry.path()))?;
            out.push(DirEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                is_dir: ft.is_dir(),
                is_symlink: ft.is_symlink(),
            });
        }
        Ok(out)
    }
    fn mkdir(&self, path: &Path, recursive: bool) -> Result<(), FsError> {
        let r = if recursive {
            std::fs::create_dir_all(path)
        } else {
            std::fs::create_dir(path)
        };
        r.map_err(|e| FsError::from_io(&e, path))
    }
    fn remove(&self, path: &Path, recursive: bool) -> Result<(), FsError> {
        let meta = std::fs::symlink_metadata(path).map_err(|e| FsError::from_io(&e, path))?;
        if meta.is_dir() {
            let r = if recursive {
                std::fs::remove_dir_all(path)
            } else {
                std::fs::remove_dir(path)
            };
            r.map_err(|e| FsError::from_io(&e, path))
        } else {
            std::fs::remove_file(path).map_err(|e| FsError::from_io(&e, path))
        }
    }
    fn copy(&self, from: &Path, to: &Path) -> Result<(), FsError> {
        std::fs::copy(from, to)
            .map(|_| ())
            .map_err(|e| FsError::from_io(&e, from))
    }
    fn read_link(&self, path: &Path) -> Result<PathBuf, FsError> {
        std::fs::read_link(path).map_err(|e| FsError::from_io(&e, path))
    }
    fn realpath(&self, path: &Path) -> Result<PathBuf, FsError> {
        std::fs::canonicalize(path).map_err(|e| FsError::from_io(&e, path))
    }
}

/// Path admission policy (security shield).
pub trait Sandbox: Send + Sync + 'static {
    /// Validates that `path` is accessible. Returns the canonicalised
    /// path on success.
    ///
    /// # Errors
    /// [`FsError`] with code `EACCES` when the path escapes the policy.
    fn admit(&self, path: &Path) -> Result<PathBuf, FsError>;
}

/// Sandbox that constrains every access to one of `roots` after path
/// canonicalisation.
#[derive(Debug, Clone)]
pub struct PathSandbox {
    roots: Vec<PathBuf>,
}

impl PathSandbox {
    /// Builds a sandbox pinned to `roots`. Each root is canonicalised
    /// once at construction; symlink swaps after this point cannot
    /// retroactively widen the sandbox.
    ///
    /// # Panics
    /// Panics when `roots` is empty.
    #[must_use]
    pub fn new(roots: Vec<PathBuf>) -> Self {
        assert!(!roots.is_empty(), "PathSandbox requires at least one root");
        let canonical = roots
            .into_iter()
            .map(|r| r.canonicalize().unwrap_or(r))
            .collect();
        Self { roots: canonical }
    }

    fn lexical_normalise(path: &Path) -> PathBuf {
        let mut out = PathBuf::new();
        for comp in path.components() {
            use std::path::Component as C;
            match comp {
                C::ParentDir => {
                    out.pop();
                }
                C::CurDir => {}
                other => out.push(other.as_os_str()),
            }
        }
        out
    }

    fn canonical_within(&self, path: &Path) -> Option<PathBuf> {
        let lexical = Self::lexical_normalise(path);
        let canon = path.canonicalize().ok().or_else(|| {
            let parent = path.parent()?.canonicalize().ok()?;
            let file = path.file_name()?;
            Some(parent.join(file))
        });
        let probe = canon.unwrap_or(lexical);
        for root in &self.roots {
            if probe.starts_with(root) {
                return Some(probe);
            }
        }
        None
    }
}

impl Sandbox for PathSandbox {
    fn admit(&self, path: &Path) -> Result<PathBuf, FsError> {
        self.canonical_within(path).ok_or_else(|| {
            FsError::new(
                "EACCES",
                format!("EACCES: path escapes sandbox ({})", path.display()),
            )
        })
    }
}

/// Aggregate OpState handle: backend + sandbox installed once at boot.
#[derive(Clone)]
pub struct FsHandle {
    backend: Arc<dyn FsBackend>,
    sandbox: Arc<dyn Sandbox>,
}

impl FsHandle {
    /// Builds a handle from any backend + sandbox.
    #[must_use]
    pub fn new(backend: Arc<dyn FsBackend>, sandbox: Arc<dyn Sandbox>) -> Self {
        Self { backend, sandbox }
    }

    /// Convenience constructor: production [`RealFs`] +
    /// [`PathSandbox`] over `roots`.
    #[must_use]
    pub fn real(roots: Vec<PathBuf>) -> Self {
        Self::new(Arc::new(RealFs), Arc::new(PathSandbox::new(roots)))
    }

    fn check(&self, path: &str) -> Result<PathBuf, FsError> {
        self.sandbox.admit(Path::new(path))
    }

    /// Sandbox-checked file read. Returns the bytes on success.
    ///
    /// # Errors
    /// `EACCES` when `path` escapes the sandbox; otherwise propagates
    /// backend I/O failures.
    pub fn read(&self, path: &str) -> Result<Vec<u8>, FsError> {
        let p = self.check(path).inspect_err(|e| log_err("read", path, e))?;
        self.backend
            .read(&p)
            .inspect_err(|e| log_err("read", path, e))
    }

    /// Sandbox-checked file write.
    ///
    /// # Errors
    /// `EACCES` when `path` escapes the sandbox; otherwise propagates
    /// backend I/O failures.
    pub fn write(&self, path: &str, data: &[u8]) -> Result<(), FsError> {
        let p = self
            .check(path)
            .inspect_err(|e| log_err("write", path, e))?;
        self.backend
            .write(&p, data)
            .inspect_err(|e| log_err("write", path, e))
    }

    /// Sandbox-checked metadata lookup.
    ///
    /// # Errors
    /// `EACCES` when `path` escapes the sandbox; otherwise propagates
    /// backend I/O failures.
    pub fn stat(&self, path: &str, follow: bool) -> Result<FsStat, FsError> {
        let p = self.check(path).inspect_err(|e| log_err("stat", path, e))?;
        self.backend
            .stat(&p, follow)
            .inspect_err(|e| log_err("stat", path, e))
    }

    /// Sandbox-checked existence probe - returns `false` when the path
    /// is inadmissible or the backend reports absent.
    #[must_use]
    pub fn exists(&self, path: &str) -> bool {
        self.check(path)
            .map(|p| self.backend.exists(&p))
            .unwrap_or(false)
    }

    /// Sandbox-checked directory listing.
    ///
    /// # Errors
    /// `EACCES` when `path` escapes the sandbox; otherwise propagates
    /// backend I/O failures.
    pub fn read_dir(&self, path: &str) -> Result<Vec<DirEntry>, FsError> {
        let p = self
            .check(path)
            .inspect_err(|e| log_err("read_dir", path, e))?;
        self.backend
            .read_dir(&p)
            .inspect_err(|e| log_err("read_dir", path, e))
    }

    /// Sandbox-checked directory creation.
    ///
    /// # Errors
    /// `EACCES` when `path` escapes the sandbox; otherwise propagates
    /// backend I/O failures.
    pub fn mkdir(&self, path: &str, recursive: bool) -> Result<(), FsError> {
        let p = self
            .check(path)
            .inspect_err(|e| log_err("mkdir", path, e))?;
        self.backend
            .mkdir(&p, recursive)
            .inspect_err(|e| log_err("mkdir", path, e))
    }

    /// Sandbox-checked file or directory removal. Missing paths are a
    /// no-op (mirrors `fs.rmSync(..., { force: true })`).
    ///
    /// # Errors
    /// `EACCES` when `path` escapes the sandbox; otherwise propagates
    /// backend I/O failures.
    pub fn remove(&self, path: &str, recursive: bool) -> Result<(), FsError> {
        let p = self
            .check(path)
            .inspect_err(|e| log_err("remove", path, e))?;
        if !self.backend.exists(&p) {
            return Ok(());
        }
        self.backend
            .remove(&p, recursive)
            .inspect_err(|e| log_err("remove", path, e))
    }

    /// Sandbox-checked copy - both endpoints must be admissible.
    ///
    /// # Errors
    /// `EACCES` when either endpoint escapes the sandbox; otherwise
    /// propagates backend I/O failures.
    pub fn copy(&self, from: &str, to: &str) -> Result<(), FsError> {
        let src = self
            .check(from)
            .inspect_err(|e| log_err("copy_src", from, e))?;
        let dst = self.check(to).inspect_err(|e| log_err("copy_dst", to, e))?;
        self.backend
            .copy(&src, &dst)
            .inspect_err(|e| log_err("copy", from, e))
    }

    /// Sandbox-checked symlink target read.
    ///
    /// # Errors
    /// `EACCES` when `path` escapes the sandbox; otherwise propagates
    /// backend I/O failures.
    pub fn read_link(&self, path: &str) -> Result<PathBuf, FsError> {
        let p = self
            .check(path)
            .inspect_err(|e| log_err("read_link", path, e))?;
        self.backend
            .read_link(&p)
            .inspect_err(|e| log_err("read_link", path, e))
    }

    /// Sandbox-checked canonical path lookup.
    ///
    /// # Errors
    /// `EACCES` when `path` escapes the sandbox; otherwise propagates
    /// backend I/O failures.
    pub fn realpath(&self, path: &str) -> Result<PathBuf, FsError> {
        let p = self
            .check(path)
            .inspect_err(|e| log_err("realpath", path, e))?;
        self.backend
            .realpath(&p)
            .inspect_err(|e| log_err("realpath", path, e))
    }
}

/// In-memory [`FsBackend`] used by unit tests.
#[derive(Debug, Default)]
pub struct MemoryFs {
    inner: Mutex<BTreeMap<PathBuf, Vec<u8>>>,
}

impl MemoryFs {
    /// Creates an empty in-memory fs.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl FsBackend for MemoryFs {
    fn read(&self, path: &Path) -> Result<Vec<u8>, FsError> {
        self.inner
            .lock()
            .expect("memfs lock")
            .get(path)
            .cloned()
            .ok_or_else(|| FsError::new("ENOENT", format!("ENOENT: {}", path.display())))
    }
    fn write(&self, path: &Path, data: &[u8]) -> Result<(), FsError> {
        self.inner
            .lock()
            .expect("memfs lock")
            .insert(path.to_path_buf(), data.to_vec());
        Ok(())
    }
    fn stat(&self, path: &Path, _follow: bool) -> Result<FsStat, FsError> {
        let map = self.inner.lock().expect("memfs lock");
        if let Some(buf) = map.get(path) {
            return Ok(FsStat {
                size: buf.len() as u64,
                is_file: true,
                is_dir: false,
                is_symlink: false,
                mtime_ms: 0.0,
                mode: 0o644,
            });
        }
        let prefix = format!("{}/", path.display());
        if map.keys().any(|k| k.to_string_lossy().starts_with(&prefix)) {
            return Ok(FsStat {
                size: 0,
                is_file: false,
                is_dir: true,
                is_symlink: false,
                mtime_ms: 0.0,
                mode: 0o755,
            });
        }
        Err(FsError::new(
            "ENOENT",
            format!("ENOENT: {}", path.display()),
        ))
    }
    fn exists(&self, path: &Path) -> bool {
        self.stat(path, true).is_ok()
    }
    fn read_dir(&self, path: &Path) -> Result<Vec<DirEntry>, FsError> {
        let map = self.inner.lock().expect("memfs lock");
        let prefix = format!("{}/", path.display());
        let mut seen: BTreeMap<String, bool> = BTreeMap::new();
        for key in map.keys() {
            let k = key.to_string_lossy();
            if let Some(rest) = k.strip_prefix(&prefix) {
                if let Some((head, _)) = rest.split_once('/') {
                    seen.entry(head.to_owned()).or_insert(true);
                } else {
                    seen.insert(rest.to_owned(), false);
                }
            }
        }
        Ok(seen
            .into_iter()
            .map(|(name, is_dir)| DirEntry {
                name,
                is_dir,
                is_symlink: false,
            })
            .collect())
    }
    fn mkdir(&self, _path: &Path, _recursive: bool) -> Result<(), FsError> {
        Ok(())
    }
    fn remove(&self, path: &Path, recursive: bool) -> Result<(), FsError> {
        let mut map = self.inner.lock().expect("memfs lock");
        if map.remove(path).is_some() {
            return Ok(());
        }
        if recursive {
            let prefix = format!("{}/", path.display());
            let keys: Vec<_> = map
                .keys()
                .filter(|k| k.to_string_lossy().starts_with(&prefix))
                .cloned()
                .collect();
            for k in keys {
                map.remove(&k);
            }
            return Ok(());
        }
        Err(FsError::new(
            "ENOENT",
            format!("ENOENT: {}", path.display()),
        ))
    }
    fn copy(&self, from: &Path, to: &Path) -> Result<(), FsError> {
        let data = self.read(from)?;
        self.write(to, &data)
    }
    fn read_link(&self, path: &Path) -> Result<PathBuf, FsError> {
        Err(FsError::new(
            "EINVAL",
            format!("EINVAL: not a link ({})", path.display()),
        ))
    }
    fn realpath(&self, path: &Path) -> Result<PathBuf, FsError> {
        Ok(path.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct AlwaysAllow;
    impl Sandbox for AlwaysAllow {
        fn admit(&self, path: &Path) -> Result<PathBuf, FsError> {
            Ok(path.to_path_buf())
        }
    }

    #[test]
    fn memory_fs_round_trips_writes_and_reads() {
        let fs = MemoryFs::new();
        let p = PathBuf::from("/a/b.txt");
        fs.write(&p, b"hello").expect("write");
        assert_eq!(fs.read(&p).expect("read"), b"hello");
        let st = fs.stat(&p, true).expect("stat");
        assert!(st.is_file && st.size == 5);
    }

    #[test]
    fn memory_fs_lists_immediate_children() {
        let fs = MemoryFs::new();
        fs.write(Path::new("/r/a.txt"), b"x").unwrap();
        fs.write(Path::new("/r/sub/b.txt"), b"y").unwrap();
        let mut names: Vec<_> = fs
            .read_dir(Path::new("/r"))
            .unwrap()
            .into_iter()
            .map(|e| (e.name, e.is_dir))
            .collect();
        names.sort();
        assert_eq!(names, vec![("a.txt".into(), false), ("sub".into(), true)]);
    }

    #[test]
    fn sandbox_admits_inside_root() {
        let dir = tempfile::tempdir().expect("tmp");
        let inside = dir.path().join("x.txt");
        std::fs::write(&inside, b"x").expect("seed");
        let sb = PathSandbox::new(vec![dir.path().to_path_buf()]);
        assert!(sb.admit(&inside).is_ok());
    }

    #[test]
    fn sandbox_blocks_paths_outside_root() {
        let dir = tempfile::tempdir().expect("tmp");
        let outside = tempfile::tempdir().expect("outside");
        let evil = outside.path().join("secret.txt");
        std::fs::write(&evil, b"x").expect("seed");
        let sb = PathSandbox::new(vec![dir.path().to_path_buf()]);
        let err = sb.admit(&evil).unwrap_err();
        assert_eq!(err.code, "EACCES");
    }

    #[test]
    fn sandbox_blocks_dotdot_traversal() {
        let dir = tempfile::tempdir().expect("tmp");
        let sb = PathSandbox::new(vec![dir.path().to_path_buf()]);
        let traversal = dir.path().join("..").join("..").join("etc").join("passwd");
        let err = sb.admit(&traversal).unwrap_err();
        assert_eq!(err.code, "EACCES");
    }

    #[test]
    fn fs_handle_combines_backend_and_sandbox() {
        let h = FsHandle::new(Arc::new(MemoryFs::new()), Arc::new(AlwaysAllow));
        let p = h.check("/x").expect("check");
        h.backend.write(&p, b"hi").expect("write");
        assert_eq!(h.backend.read(&p).expect("read"), b"hi");
    }

    #[test]
    fn fs_error_maps_io_kinds_to_node_codes() {
        let dir = tempfile::tempdir().expect("tmp");
        let err = std::fs::read(dir.path().join("nope")).unwrap_err();
        let mapped = FsError::from_io(&err, dir.path());
        assert_eq!(mapped.code, "ENOENT");
    }
}
