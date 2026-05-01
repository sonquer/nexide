//! Subset of Next.js `images` config honoured by the native optimizer.
//!
//! Loaded once from `<app>/.next/required-server-files.json` at server
//! start. Field names mirror Next exactly so JSON deserialization works
//! against the production config dump unchanged.

use std::path::Path;

use serde::Deserialize;

/// Cache version embedded in the cache key. Bumped whenever the
/// pipeline produces output that's not byte-equivalent to the previous
/// release. Mirrors `CACHE_VERSION = 4` in upstream image-optimizer.
pub(crate) const CACHE_VERSION: u32 = 4;

/// Default minimum cache TTL, matching `image-config.js:57`.
pub(crate) const DEFAULT_MIN_CACHE_TTL: u64 = 14_400;

/// Default redirect cap for upstream fetches.
pub(crate) const DEFAULT_MAX_REDIRECTS: u32 = 3;

/// Default upstream response body cap (50 MB).
pub(crate) const DEFAULT_MAX_RESPONSE_BODY: u64 = 50_000_000;

/// Default `Content-Security-Policy` for image responses.
pub(crate) const DEFAULT_CSP: &str = "script-src 'none'; frame-src 'none'; sandbox;";

/// Parsed `images` block from `required-server-files.json`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageConfig {
    /// Allowed `w` values for device-targeted images.
    #[serde(default = "default_device_sizes")]
    pub device_sizes: Vec<u32>,
    /// Allowed `w` values for fixed-size images.
    #[serde(default = "default_image_sizes")]
    pub image_sizes: Vec<u32>,
    /// Allowed `q` values. Empty array still permits `75` per upstream.
    #[serde(default)]
    pub qualities: Option<Vec<u8>>,
    /// Format preference order - `image/avif`, `image/webp`, …
    #[serde(default = "default_formats")]
    pub formats: Vec<String>,
    /// Cache TTL floor, in seconds.
    #[serde(default = "default_min_cache_ttl")]
    pub minimum_cache_ttl: u64,
    /// Maximum number of redirects honoured for upstream URLs.
    #[serde(default = "default_max_redirects")]
    pub maximum_redirects: u32,
    /// Maximum upstream response body, in bytes.
    #[serde(default = "default_max_response_body")]
    pub maximum_response_body: u64,
    /// Whether to allow upstream URLs that resolve to private IPs.
    #[serde(default)]
    pub dangerously_allow_local_ip: bool,
    /// Whether SVG sources are accepted (without resize).
    #[serde(default)]
    pub dangerously_allow_svg: bool,
    /// `inline` or `attachment`.
    #[serde(default = "default_disposition")]
    pub content_disposition_type: String,
    /// CSP header attached to image responses.
    #[serde(default = "default_csp")]
    pub content_security_policy: String,
    /// Local URL allowlist. Defaults to a single `**` pattern.
    #[serde(default = "default_local_patterns")]
    pub local_patterns: Vec<LocalPattern>,
    /// Remote URL allowlist. Empty means: deny all remote URLs.
    #[serde(default)]
    pub remote_patterns: Vec<RemotePattern>,
    /// Deprecated alias for `remote_patterns` host-only entries.
    #[serde(default)]
    pub domains: Vec<String>,
    /// `loader` field; only `default` opts into this route.
    #[serde(default = "default_loader")]
    pub loader: String,
    /// When `true`, the route should 404 (caller already enforces).
    #[serde(default)]
    pub unoptimized: bool,
}

/// Glob pattern entry for `localPatterns`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct LocalPattern {
    #[serde(default)]
    pub pathname: Option<String>,
    #[serde(default)]
    pub search: Option<String>,
}

/// Glob pattern entry for `remotePatterns`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RemotePattern {
    #[serde(default)]
    pub protocol: Option<String>,
    #[serde(default)]
    pub hostname: Option<String>,
    #[serde(default)]
    pub port: Option<String>,
    #[serde(default)]
    pub pathname: Option<String>,
    #[serde(default)]
    pub search: Option<String>,
}

fn default_device_sizes() -> Vec<u32> {
    vec![640, 750, 828, 1080, 1200, 1920, 2048, 3840]
}

fn default_image_sizes() -> Vec<u32> {
    vec![16, 32, 48, 64, 96, 128, 256, 384]
}

fn default_formats() -> Vec<String> {
    vec!["image/webp".to_owned()]
}

const fn default_min_cache_ttl() -> u64 {
    DEFAULT_MIN_CACHE_TTL
}

const fn default_max_redirects() -> u32 {
    DEFAULT_MAX_REDIRECTS
}

const fn default_max_response_body() -> u64 {
    DEFAULT_MAX_RESPONSE_BODY
}

fn default_disposition() -> String {
    "attachment".to_owned()
}

fn default_csp() -> String {
    DEFAULT_CSP.to_owned()
}

fn default_local_patterns() -> Vec<LocalPattern> {
    vec![LocalPattern {
        pathname: Some("**".to_owned()),
        search: None,
    }]
}

fn default_loader() -> String {
    "default".to_owned()
}

impl Default for ImageConfig {
    fn default() -> Self {
        Self {
            device_sizes: default_device_sizes(),
            image_sizes: default_image_sizes(),
            qualities: None,
            formats: default_formats(),
            minimum_cache_ttl: DEFAULT_MIN_CACHE_TTL,
            maximum_redirects: DEFAULT_MAX_REDIRECTS,
            maximum_response_body: DEFAULT_MAX_RESPONSE_BODY,
            dangerously_allow_local_ip: false,
            dangerously_allow_svg: false,
            content_disposition_type: "attachment".to_owned(),
            content_security_policy: DEFAULT_CSP.to_owned(),
            local_patterns: default_local_patterns(),
            remote_patterns: Vec::new(),
            domains: Vec::new(),
            loader: "default".to_owned(),
            unoptimized: false,
        }
    }
}

impl ImageConfig {
    /// Loads `images` from the standalone bundle's
    /// `required-server-files.json`. The file is emitted at the
    /// **workspace root** of the bundle (`<workspace>/.next/required-server-files.json`),
    /// not under `.next/server/app`. Callers pass the resolved
    /// `app_dir` (`.next/server/app`); we walk up two parents to get
    /// to `.next/`.
    ///
    /// Returns [`Self::default`] when the file is absent or malformed -
    /// the optimizer should never block server startup.
    #[must_use]
    pub fn from_app_dir(app_dir: &Path) -> Self {
        let dot_next = app_dir
            .parent()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .unwrap_or_else(|| app_dir.join(".next"));
        let mut candidates = vec![dot_next.join("required-server-files.json")];
        candidates.push(app_dir.join(".next").join("required-server-files.json"));
        let text = candidates
            .iter()
            .find_map(|p| std::fs::read_to_string(p).ok());
        let Some(text) = text else {
            return Self::default();
        };
        let Ok(root) = serde_json::from_str::<serde_json::Value>(&text) else {
            return Self::default();
        };
        root.get("config")
            .and_then(|c| c.get("images"))
            .and_then(|v| serde_json::from_value::<Self>(v.clone()).ok())
            .unwrap_or_default()
    }

    /// Whether `width` is on the configured allowlist.
    #[must_use]
    pub fn allows_width(&self, width: u32) -> bool {
        self.device_sizes.contains(&width) || self.image_sizes.contains(&width)
    }

    /// Whether `quality` is on the configured allowlist.
    /// When `qualities` is unset, accepts any value 1..=100; when set,
    /// requires exact membership (matching upstream behaviour).
    #[must_use]
    pub fn allows_quality(&self, quality: u8) -> bool {
        match &self.qualities {
            None => true,
            Some(list) => list.contains(&quality),
        }
    }

    /// Whether the route is reachable. Mirrors the early-exit gate in
    /// `next-server.js:198` - only `loader === 'default'` and not
    /// `unoptimized` makes the route resolvable.
    #[must_use]
    pub fn route_enabled(&self) -> bool {
        self.loader == "default" && !self.unoptimized
    }
}
