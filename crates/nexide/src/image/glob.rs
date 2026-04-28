//! Picomatch-compatible glob matcher used by `localPatterns` and
//! `remotePatterns`.
//!
//! Implements the subset upstream relies on: literal characters, `?`
//! (single non-separator char), `*` (any number of non-separator
//! chars), `**` (any number of any chars including separators), and
//! `{a,b,c}` brace alternatives. Separator: `/`.

use super::config::{LocalPattern, RemotePattern};

/// Matches `text` against `pattern`. `dot` controls whether leading
/// dots in segments may be matched by `*`/`**`/`?`. The Next.js call
/// sites we mirror always pass `dot: true`.
pub(crate) fn matches(pattern: &str, text: &str, dot: bool) -> bool {
    matches_impl(pattern.as_bytes(), text.as_bytes(), 0, 0, dot, true)
}

fn matches_impl(p: &[u8], t: &[u8], pi: usize, ti: usize, dot: bool, at_seg: bool) -> bool {
    if pi == p.len() {
        return ti == t.len();
    }
    match p[pi] {
        b'*' if pi + 1 < p.len() && p[pi + 1] == b'*' => {
            let np = pi + 2;
            let np = if np < p.len() && p[np] == b'/' {
                np + 1
            } else {
                np
            };
            for end in ti..=t.len() {
                if matches_impl(
                    p,
                    t,
                    np,
                    end,
                    dot,
                    end == 0 || t.get(end - 1) == Some(&b'/'),
                ) {
                    return true;
                }
            }
            false
        }
        b'*' => {
            for end in ti..=t.len() {
                if (end..t.len()).any(|j| t[j] == b'/') && end == ti {
                    continue;
                }
                let slice = &t[ti..end];
                if slice.contains(&b'/') {
                    break;
                }
                if !dot && at_seg && slice.first() == Some(&b'.') {
                    continue;
                }
                if matches_impl(p, t, pi + 1, end, dot, false) {
                    return true;
                }
            }
            false
        }
        b'?' => {
            if ti >= t.len() || t[ti] == b'/' {
                return false;
            }
            if !dot && at_seg && t[ti] == b'.' {
                return false;
            }
            matches_impl(p, t, pi + 1, ti + 1, dot, false)
        }
        b'{' => {
            let mut end = pi + 1;
            let mut depth = 1u32;
            while end < p.len() && depth > 0 {
                match p[end] {
                    b'{' => depth += 1,
                    b'}' => depth -= 1,
                    _ => {}
                }
                if depth > 0 {
                    end += 1;
                }
            }
            if end >= p.len() {
                return false;
            }
            for alt in split_brace_alts(&p[pi + 1..end]) {
                let mut combined = Vec::with_capacity(alt.len() + p.len() - end - 1);
                combined.extend_from_slice(&alt);
                combined.extend_from_slice(&p[end + 1..]);
                if matches_impl(&combined, t, 0, ti, dot, at_seg) {
                    return true;
                }
            }
            false
        }
        b'\\' if pi + 1 < p.len() => {
            if ti < t.len() && t[ti] == p[pi + 1] {
                matches_impl(p, t, pi + 2, ti + 1, dot, t[ti] == b'/')
            } else {
                false
            }
        }
        c => {
            if ti < t.len() && t[ti] == c {
                matches_impl(p, t, pi + 1, ti + 1, dot, c == b'/')
            } else {
                false
            }
        }
    }
}

fn split_brace_alts(s: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut depth = 0u32;
    let mut buf = Vec::new();
    for &b in s {
        match b {
            b'{' => {
                depth += 1;
                buf.push(b);
            }
            b'}' => {
                depth = depth.saturating_sub(1);
                buf.push(b);
            }
            b',' if depth == 0 => {
                out.push(std::mem::take(&mut buf));
            }
            _ => buf.push(b),
        }
    }
    out.push(buf);
    out
}

/// Returns `true` when `path`/`search` are admitted by at least one
/// local-pattern entry. Empty `patterns` admits everything.
pub(crate) fn local_pattern_matches(patterns: &[LocalPattern], path: &str, search: &str) -> bool {
    if patterns.is_empty() {
        return true;
    }
    patterns.iter().any(|p| {
        let path_pat = p.pathname.as_deref().unwrap_or("**");
        let path_ok = matches(path_pat, path, true);
        let search_ok = match p.search.as_deref() {
            Some(s) => s == search,
            None => true,
        };
        path_ok && search_ok
    })
}

/// Returns `true` when the parsed URL is admitted by `patterns` or by
/// the deprecated `domains` allowlist (host-only match).
pub(crate) fn remote_pattern_matches(
    patterns: &[RemotePattern],
    domains: &[String],
    url: &url::Url,
) -> bool {
    let host = url.host_str().unwrap_or("");
    if !host.is_empty() && domains.iter().any(|d| d == host) {
        return true;
    }
    patterns.iter().any(|p| remote_one_matches(p, url))
}

fn remote_one_matches(pattern: &RemotePattern, url: &url::Url) -> bool {
    if let Some(proto) = pattern.protocol.as_deref() {
        let want = proto.trim_end_matches(':');
        if want != url.scheme() {
            return false;
        }
    }
    let host = url.host_str().unwrap_or("");
    let host_pat = pattern.hostname.as_deref().unwrap_or("");
    if host_pat.is_empty() || !matches(host_pat, host, false) {
        return false;
    }
    if let Some(port) = pattern.port.as_deref()
        && !port.is_empty()
    {
        let actual = url.port().map(|p| p.to_string()).unwrap_or_default();
        if port != actual {
            return false;
        }
    }
    let path_pat = pattern.pathname.as_deref().unwrap_or("**");
    if !matches(path_pat, url.path(), true) {
        return false;
    }
    if let Some(search) = pattern.search.as_deref() {
        let actual = url.query().unwrap_or("");
        if search != actual {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globstar_matches_any_depth() {
        assert!(matches("**", "/a/b/c.png", true));
        assert!(matches("/img/**", "/img/a/b.png", true));
        assert!(matches("/img/**/*.png", "/img/sub/x.png", true));
        assert!(!matches("/img/**", "/other/a.png", true));
    }

    #[test]
    fn star_does_not_cross_slash() {
        assert!(matches("/a/*.png", "/a/b.png", true));
        assert!(!matches("/a/*.png", "/a/b/c.png", true));
    }

    #[test]
    fn brace_alternatives() {
        assert!(matches("/img/*.{png,jpg}", "/img/a.png", true));
        assert!(matches("/img/*.{png,jpg}", "/img/a.jpg", true));
        assert!(!matches("/img/*.{png,jpg}", "/img/a.gif", true));
    }

    #[test]
    fn host_wildcards() {
        assert!(matches("*.example.com", "img.example.com", false));
        assert!(matches("**.example.com", "deep.img.example.com", false));
        assert!(!matches("*.example.com", "example.com", false));
    }
}
