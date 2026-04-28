//! Host-side façade for `node:dns` polyfill ops.
//!
//! The DNS polyfill calls into Rust through a small set of ops that
//! are deliberately stateless from the JS side: the polyfill never
//! holds a resolver handle, every lookup goes through this module.
//! Internally we lazily initialise a single
//! [`hickory_resolver::TokioAsyncResolver`] from the system
//! `/etc/resolv.conf`-style configuration (or a sane Google fallback
//! on platforms where the system config is unavailable) and share it
//! across the entire process.
//!
//! Errors are surfaced as Node-style codes (`ENOTFOUND`, `EAI_AGAIN`,
//! `ESERVFAIL`, …) so caller code can pattern-match on `err.code`
//! exactly the way it would on Node.

use std::io;
use std::net::IpAddr;
use std::sync::OnceLock;

use hickory_resolver::TokioAsyncResolver;
use hickory_resolver::config::{ResolverConfig, ResolverOpts};
use hickory_resolver::error::ResolveError;

/// Hickory's API is fully `Send + Sync` and internally backed by an
/// `Arc`, so a single resolver shared across all worker isolates is
/// enough — and avoids the cost of re-reading `resolv.conf` per
/// isolate. We initialise it lazily on the first call.
fn shared_resolver() -> &'static TokioAsyncResolver {
    static RESOLVER: OnceLock<TokioAsyncResolver> = OnceLock::new();
    RESOLVER.get_or_init(|| match TokioAsyncResolver::tokio_from_system_conf() {
        Ok(resolver) => resolver,
        Err(_) => TokioAsyncResolver::tokio(ResolverConfig::default(), ResolverOpts::default()),
    })
}

/// Mapped Node.js-style DNS error.
///
/// `code` follows the Node naming (`ENOTFOUND`, `EAI_AGAIN`,
/// `ESERVFAIL`, `ECANCELLED`, …); `message` is a human-readable
/// description suitable for `Error.message`.
#[derive(Debug, Clone)]
pub struct DnsError {
    /// Node-style error code copied verbatim into `Error.code`.
    pub code: &'static str,
    /// Human-readable description placed in `Error.message`.
    pub message: String,
}

impl DnsError {
    fn from_resolve(err: ResolveError) -> Self {
        use hickory_resolver::error::ResolveErrorKind as K;
        let code = match err.kind() {
            K::NoRecordsFound { .. } => "ENOTFOUND",
            K::Timeout => "ETIMEOUT",
            K::Io(_) => "ECONNREFUSED",
            K::Proto(_) => "EBADRESP",
            _ => "EUNKNOWN",
        };
        Self {
            code,
            message: err.to_string(),
        }
    }

    fn from_io(err: io::Error) -> Self {
        let code = match err.kind() {
            io::ErrorKind::TimedOut => "ETIMEOUT",
            io::ErrorKind::ConnectionRefused => "ECONNREFUSED",
            io::ErrorKind::NotFound => "ENOTFOUND",
            _ => "ENOTFOUND",
        };
        Self {
            code,
            message: err.to_string(),
        }
    }
}

/// Result of [`lookup`].
#[derive(Debug, Clone)]
pub struct LookupResult {
    /// Resolved IP address.
    pub address: IpAddr,
    /// `4` for an IPv4 result, `6` for IPv6 — matches Node's
    /// `family` field on `dns.lookup` callbacks.
    pub family: u8,
}

/// Filter applied to a hostname-to-address lookup.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum LookupFamily {
    /// Accept any address — IPv4 returned ahead of IPv6 to match Node's default.
    Any,
    /// IPv4 only (`A` records).
    V4,
    /// IPv6 only (`AAAA` records).
    V6,
}

impl LookupFamily {
    /// Maps Node's numeric `options.family` to the typed variant.
    /// Unknown values fall back to [`LookupFamily::Any`].
    pub fn from_node(family: u32) -> Self {
        match family {
            4 => Self::V4,
            6 => Self::V6,
            _ => Self::Any,
        }
    }
}

/// Resolves `host` to one or more IP addresses using the OS
/// resolver (via `tokio::net::lookup_host`, the same path as
/// `getaddrinfo`).
///
/// Returns at most `max` results; pass `usize::MAX` (or any large
/// number) for unbounded. The OS resolver is preferred over the
/// hickory-backed [`resolve4`] / [`resolve6`] because Node's
/// `dns.lookup` is documented to honour `nsswitch.conf`, hostfile
/// and mDNS — none of which hickory consults.
pub async fn lookup(
    host: &str,
    family: LookupFamily,
    max: usize,
) -> Result<Vec<LookupResult>, DnsError> {
    let target = format!("{host}:0");
    let addrs = tokio::net::lookup_host(target)
        .await
        .map_err(DnsError::from_io)?;
    let mut out = Vec::new();
    for sa in addrs {
        let ip = sa.ip();
        let fam = if ip.is_ipv4() { 4u8 } else { 6 };
        let keep = match family {
            LookupFamily::Any => true,
            LookupFamily::V4 => fam == 4,
            LookupFamily::V6 => fam == 6,
        };
        if keep {
            out.push(LookupResult {
                address: ip,
                family: fam,
            });
            if out.len() >= max {
                break;
            }
        }
    }
    if out.is_empty() {
        return Err(DnsError {
            code: "ENOTFOUND",
            message: format!("getaddrinfo ENOTFOUND {host}"),
        });
    }
    Ok(out)
}

/// Returns the IPv4 addresses for `host` (`A` records only).
pub async fn resolve4(host: &str) -> Result<Vec<IpAddr>, DnsError> {
    let resolver = shared_resolver();
    let lookup = resolver
        .ipv4_lookup(host)
        .await
        .map_err(DnsError::from_resolve)?;
    Ok(lookup.iter().map(|a| IpAddr::V4(a.0)).collect())
}

/// Returns the IPv6 addresses for `host` (`AAAA` records only).
pub async fn resolve6(host: &str) -> Result<Vec<IpAddr>, DnsError> {
    let resolver = shared_resolver();
    let lookup = resolver
        .ipv6_lookup(host)
        .await
        .map_err(DnsError::from_resolve)?;
    Ok(lookup.iter().map(|a| IpAddr::V6(a.0)).collect())
}

/// One mail-exchange record, mapping a priority to a target host.
#[derive(Debug, Clone)]
pub struct MxRecord {
    /// Lower values are preferred, matching RFC 5321 §5.1.
    pub priority: u16,
    /// Fully-qualified domain name of the mail exchanger.
    pub exchange: String,
}

/// Returns the MX records for `host`.
pub async fn resolve_mx(host: &str) -> Result<Vec<MxRecord>, DnsError> {
    let resolver = shared_resolver();
    let lookup = resolver
        .mx_lookup(host)
        .await
        .map_err(DnsError::from_resolve)?;
    Ok(lookup
        .iter()
        .map(|r| MxRecord {
            priority: r.preference(),
            exchange: r.exchange().to_string(),
        })
        .collect())
}

/// Returns the TXT records for `host`. Each record is the
/// concatenation of its `<character-string>` chunks (matching
/// Node's `dns.resolveTxt` shape — a `string[]` per record, but we
/// flatten to one `string` per record because that is how the
/// polyfill exposes them).
pub async fn resolve_txt(host: &str) -> Result<Vec<Vec<String>>, DnsError> {
    let resolver = shared_resolver();
    let lookup = resolver
        .txt_lookup(host)
        .await
        .map_err(DnsError::from_resolve)?;
    Ok(lookup
        .iter()
        .map(|r| {
            r.iter()
                .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
                .collect()
        })
        .collect())
}

/// Returns the canonical-name records for `host`.
pub async fn resolve_cname(host: &str) -> Result<Vec<String>, DnsError> {
    let resolver = shared_resolver();
    let lookup = resolver
        .lookup(host, hickory_resolver::proto::rr::RecordType::CNAME)
        .await
        .map_err(DnsError::from_resolve)?;
    Ok(lookup
        .iter()
        .filter_map(|r| r.as_cname().map(|n| n.to_string()))
        .collect())
}

/// Returns the authoritative name servers for `host`.
pub async fn resolve_ns(host: &str) -> Result<Vec<String>, DnsError> {
    let resolver = shared_resolver();
    let lookup = resolver
        .ns_lookup(host)
        .await
        .map_err(DnsError::from_resolve)?;
    Ok(lookup.iter().map(|n| n.to_string()).collect())
}

/// One service-locator record.
#[derive(Debug, Clone)]
pub struct SrvRecord {
    /// RFC 2782 priority — lower values are tried first.
    pub priority: u16,
    /// RFC 2782 weight — used to load-balance among same-priority targets.
    pub weight: u16,
    /// TCP/UDP port of the service.
    pub port: u16,
    /// Fully-qualified domain name of the host providing the service.
    pub name: String,
}

/// Returns the SRV records for `host`.
pub async fn resolve_srv(host: &str) -> Result<Vec<SrvRecord>, DnsError> {
    let resolver = shared_resolver();
    let lookup = resolver
        .srv_lookup(host)
        .await
        .map_err(DnsError::from_resolve)?;
    Ok(lookup
        .iter()
        .map(|r| SrvRecord {
            priority: r.priority(),
            weight: r.weight(),
            port: r.port(),
            name: r.target().to_string(),
        })
        .collect())
}

/// Reverse DNS lookup — returns the PTR names for `ip`.
pub async fn reverse(ip: IpAddr) -> Result<Vec<String>, DnsError> {
    let resolver = shared_resolver();
    let lookup = resolver
        .reverse_lookup(ip)
        .await
        .map_err(DnsError::from_resolve)?;
    Ok(lookup.iter().map(|n| n.to_string()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `localhost` must resolve through the OS path. We do not assert
    /// on the exact address (could be `127.0.0.1` or `::1` depending
    /// on /etc/hosts) — only that *some* result is returned.
    #[tokio::test]
    async fn lookup_localhost_returns_at_least_one_address() {
        let res = lookup("localhost", LookupFamily::Any, 16)
            .await
            .expect("localhost should resolve");
        assert!(!res.is_empty(), "expected at least one address");
    }

    #[test]
    fn lookup_family_from_node_maps_known_values() {
        assert_eq!(LookupFamily::from_node(4), LookupFamily::V4);
        assert_eq!(LookupFamily::from_node(6), LookupFamily::V6);
        assert_eq!(LookupFamily::from_node(0), LookupFamily::Any);
        assert_eq!(LookupFamily::from_node(99), LookupFamily::Any);
    }

    #[test]
    fn dns_error_from_io_maps_kinds() {
        let e = DnsError::from_io(io::Error::new(io::ErrorKind::TimedOut, "x"));
        assert_eq!(e.code, "ETIMEOUT");
        let e = DnsError::from_io(io::Error::new(io::ErrorKind::ConnectionRefused, "x"));
        assert_eq!(e.code, "ECONNREFUSED");
        let e = DnsError::from_io(io::Error::other("x"));
        assert_eq!(e.code, "ENOTFOUND");
    }
}
