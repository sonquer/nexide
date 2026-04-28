//! On-disk cache for optimized image bytes.
//!
//! Mirrors upstream layout in `image-optimizer.js`:
//!   - directory: `<app>/.next/cache/images/<cacheKey>/`
//!   - filename : `<maxAge>.<expireAt>.<etag>.<upstreamEtag>.<ext>`
//!
//! The cache key is the SHA256 of `[CACHE_VERSION, href, width,
//! quality, mimeType]`, base64url-encoded. Two requests differing only
//! in `Accept` get distinct entries because the chosen output mime
//! type is part of the key.

use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};

use super::config::CACHE_VERSION;
use super::pipeline::OutputFormat;

#[derive(Debug, Clone)]
pub(crate) struct CacheEntry {
    pub(crate) key: String,
    pub(crate) max_age: u64,
    pub(crate) expire_at_ms: u128,
    pub(crate) etag: String,
    pub(crate) upstream_etag: String,
    pub(crate) extension: &'static str,
    pub(crate) bytes: Vec<u8>,
}

/// Computes the cache key for `(href, width, quality, mime)`.
pub(crate) fn cache_key(href: &str, width: u32, quality: u8, mime: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(CACHE_VERSION.to_string().as_bytes());
    hasher.update(href.as_bytes());
    hasher.update(width.to_string().as_bytes());
    hasher.update(quality.to_string().as_bytes());
    hasher.update(mime.as_bytes());
    URL_SAFE_NO_PAD.encode(hasher.finalize())
}

/// Computes the SHA256 etag of an optimized image buffer.
pub(crate) fn buffer_etag(buf: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(buf);
    URL_SAFE_NO_PAD.encode(hasher.finalize())
}

/// Encodes an upstream etag into base64url so it round-trips through
/// the filename safely.
pub(crate) fn encode_upstream_etag(raw: &str) -> String {
    URL_SAFE_NO_PAD.encode(raw.as_bytes())
}

/// Returns the directory under which a cache key lives.
pub(crate) fn dir(app_dir: &Path, key: &str) -> PathBuf {
    app_dir.join(".next").join("cache").join("images").join(key)
}

/// Reads the freshest entry for `key`. Returns `None` when the
/// directory is empty or no readable file is present.
pub(crate) fn read(app_dir: &Path, key: &str) -> Option<CacheEntry> {
    let dir_path = dir(app_dir, key);
    let read_dir = std::fs::read_dir(&dir_path).ok()?;
    let mut newest: Option<(u128, CacheEntry)> = None;
    for entry in read_dir.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        let Some(parsed) = parse_filename(name) else {
            continue;
        };
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let candidate = CacheEntry {
            key: key.to_owned(),
            max_age: parsed.max_age,
            expire_at_ms: parsed.expire_at_ms,
            etag: parsed.etag,
            upstream_etag: parsed.upstream_etag,
            extension: parsed.extension,
            bytes,
        };
        match &newest {
            Some((seen, _)) if *seen >= candidate.expire_at_ms => {}
            _ => newest = Some((candidate.expire_at_ms, candidate)),
        }
    }
    newest.map(|(_, e)| e)
}

/// Writes `entry` to disk under `<app>/.next/cache/images/<key>/`.
pub(crate) fn write(app_dir: &Path, entry: &CacheEntry) -> std::io::Result<PathBuf> {
    let dir_path = dir(app_dir, &entry.key);
    std::fs::create_dir_all(&dir_path)?;
    let filename = format!(
        "{}.{}.{}.{}.{}",
        entry.max_age, entry.expire_at_ms, entry.etag, entry.upstream_etag, entry.extension
    );
    let path = dir_path.join(filename);
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, &entry.bytes)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

struct ParsedName {
    max_age: u64,
    expire_at_ms: u128,
    etag: String,
    upstream_etag: String,
    extension: &'static str,
}

fn parse_filename(name: &str) -> Option<ParsedName> {
    let parts: Vec<&str> = name.split('.').collect();
    if parts.len() != 5 {
        return None;
    }
    let max_age: u64 = parts[0].parse().ok()?;
    let expire_at_ms: u128 = parts[1].parse().ok()?;
    let etag = parts[2].to_owned();
    let upstream_etag = parts[3].to_owned();
    let extension = match parts[4] {
        "webp" => "webp",
        "jpg" => "jpg",
        "png" => "png",
        "gif" => "gif",
        "svg" => "svg",
        _ => return None,
    };
    Some(ParsedName {
        max_age,
        expire_at_ms,
        etag,
        upstream_etag,
        extension,
    })
}

/// Convenience: derives the on-disk extension from the chosen output
/// format. Bypassed sources keep their original extension upstream;
/// callers handle that explicitly.
pub(crate) const fn extension_for(format: OutputFormat) -> &'static str {
    format.extension()
}
