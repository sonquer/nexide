//! Friendly, self-contained error pages for the HTTP shield.
//!
//! Every response is content-negotiated against the request's `Accept`
//! header so that browsers see polished HTML, API consumers see a small
//! JSON envelope and `curl` (or anything else) keeps the legacy plain
//! text. Pages are zero-dependency: inline CSS, inline SVG, no external
//! fetches, safe to render even when the upstream engine is dead.

use axum::body::Body;
use axum::http::header::{ACCEPT, CACHE_CONTROL, CONTENT_TYPE, HeaderMap, HeaderValue};
use axum::http::{Response, StatusCode};

/// Negotiated representation for an error response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wants {
    Html,
    Json,
    Text,
}

fn negotiate(headers: Option<&HeaderMap>) -> Wants {
    let Some(headers) = headers else {
        return Wants::Html;
    };
    let Some(accept) = headers.get(ACCEPT).and_then(|v| v.to_str().ok()) else {
        return Wants::Html;
    };
    let lower = accept.to_ascii_lowercase();
    if lower.contains("text/html") {
        Wants::Html
    } else if lower.contains("application/json") {
        Wants::Json
    } else if lower.contains("*/*") || lower.is_empty() {
        Wants::Html
    } else {
        Wants::Text
    }
}

/// Builds an error response from a status code, optionally tailored by
/// the original request headers (used for content negotiation) and an
/// internal `detail` string that will be exposed only in the JSON
/// envelope (HTML/text intentionally hide it from end users).
pub(super) fn render(
    status: StatusCode,
    request_headers: Option<&HeaderMap>,
    detail: Option<&str>,
) -> Response<Body> {
    let copy = copy_for(status);
    let mode = negotiate(request_headers);
    let (content_type, body) = match mode {
        Wants::Html => (
            HeaderValue::from_static("text/html; charset=utf-8"),
            Body::from(render_html(status, &copy)),
        ),
        Wants::Json => (
            HeaderValue::from_static("application/json; charset=utf-8"),
            Body::from(render_json(status, &copy, detail)),
        ),
        Wants::Text => (
            HeaderValue::from_static("text/plain; charset=utf-8"),
            Body::from(render_text(status, &copy)),
        ),
    };
    let mut response = Response::new(body);
    *response.status_mut() = status;
    response.headers_mut().insert(CONTENT_TYPE, content_type);
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[derive(Debug, Clone, Copy)]
struct Copy<'a> {
    title: &'a str,
    summary: &'a str,
    advice: &'a str,
    accent: &'a str,
}

const fn copy_for(status: StatusCode) -> Copy<'static> {
    match status.as_u16() {
        400 => Copy {
            title: "We couldn't read this request",
            summary: "Something about the request didn't look quite right on our side.",
            advice: "Try refreshing the page. If the link came from somewhere else, double-check it for typos.",
            accent: "#f59e0b",
        },
        401 => Copy {
            title: "You need to sign in",
            summary: "This page is only available to signed-in users.",
            advice: "Sign in and try again. If you already are signed in, your session may have expired.",
            accent: "#0ea5e9",
        },
        403 => Copy {
            title: "This area isn't open to you",
            summary: "You're signed in, but this page isn't part of what your account can access.",
            advice: "If you think you should have access, contact whoever set up the account.",
            accent: "#0ea5e9",
        },
        404 => Copy {
            title: "We can't find that page",
            summary: "The link may be old, or the page may have moved.",
            advice: "Try the home page, or use search to find what you were looking for.",
            accent: "#6366f1",
        },
        408 | 504 => Copy {
            title: "This is taking longer than expected",
            summary: "The page didn't finish loading in time. The slowdown is on our side, not yours.",
            advice: "Give it a moment and try again - we're already working on it.",
            accent: "#f97316",
        },
        413 => Copy {
            title: "That request was too large to handle",
            summary: "The data you sent is bigger than what we accept in one go.",
            advice: "Try splitting it into smaller pieces or compressing it before resending.",
            accent: "#f59e0b",
        },
        429 => Copy {
            title: "Too many requests in a short time",
            summary: "We're rate-limiting traffic to keep things fair for everyone.",
            advice: "Wait a few seconds and try again. Automated tools should slow down their request rate.",
            accent: "#f59e0b",
        },
        500 => Copy {
            title: "Something went wrong on our end",
            summary: "An unexpected error stopped this page from rendering. This isn't your fault.",
            advice: "We've been notified and are looking into it. Reloading in a moment usually helps.",
            accent: "#ef4444",
        },
        501 => Copy {
            title: "We haven't built this yet",
            summary: "This route exists, but the feature behind it isn't ready.",
            advice: "Check back soon - if you were expecting this to work, let support know.",
            accent: "#6366f1",
        },
        502 => Copy {
            title: "We can't reach our backend right now",
            summary: "An upstream service didn't respond the way we expected. Nothing about this is on you.",
            advice: "Reload in a few seconds. If it sticks around, our on-call team is already on it.",
            accent: "#ef4444",
        },
        503 => Copy {
            title: "We're warming up",
            summary: "The service is starting or briefly unavailable. This is on us, not on you.",
            advice: "Hang tight and try again in a few seconds.",
            accent: "#f59e0b",
        },
        _ => Copy {
            title: "Something didn't go as planned",
            summary: "We hit an unexpected issue while handling your request. This isn't your fault.",
            advice: "Reloading the page usually helps. If it doesn't, please get in touch with support.",
            accent: "#6366f1",
        },
    }
}

fn render_html(status: StatusCode, c: &Copy<'_>) -> String {
    let code = status.as_u16();
    let phrase = status.canonical_reason().unwrap_or("Error");
    format!(
        concat!(
            "<!doctype html><html lang=\"en\"><head>",
            "<meta charset=\"utf-8\">",
            "<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">",
            "<meta name=\"robots\" content=\"noindex\">",
            "<meta name=\"color-scheme\" content=\"light dark\">",
            "<title>{code} {phrase}</title>",
            "<style>",
            ":root{{--bg:#f8fafc;--fg:#0f172a;--muted:#475569;--card:#ffffffd9;",
            "--border:#e2e8f0;--accent:{accent};--accent-soft:{accent}1f}}",
            "@media (prefers-color-scheme:dark){{:root{{--bg:#020617;--fg:#f1f5f9;",
            "--muted:#94a3b8;--card:#0f172abf;--border:#1e293b;--accent-soft:{accent}33}}}}",
            "*{{box-sizing:border-box}}html,body{{height:100%}}",
            "body{{margin:0;background:radial-gradient(1200px 600px at 0% 0%,var(--accent-soft),transparent 60%),",
            "radial-gradient(900px 500px at 100% 100%,var(--accent-soft),transparent 55%),var(--bg);",
            "color:var(--fg);font:16px/1.55 ui-sans-serif,system-ui,-apple-system,Segoe UI,Inter,Roboto,sans-serif;",
            "display:grid;place-items:center;padding:32px 20px}}",
            ".card{{width:min(560px,100%);background:var(--card);border:1px solid var(--border);",
            "border-radius:18px;padding:40px 36px;backdrop-filter:saturate(160%) blur(8px);",
            "box-shadow:0 24px 60px -28px rgba(15,23,42,.35)}}",
            ".eyebrow{{display:inline-flex;align-items:center;gap:8px;color:var(--accent);",
            "font-weight:600;font-size:13px;letter-spacing:.06em;text-transform:uppercase;margin:0 0 20px}}",
            ".dot{{width:8px;height:8px;border-radius:99px;background:var(--accent);",
            "box-shadow:0 0 0 4px var(--accent-soft);animation:pulse 2.4s ease-in-out infinite}}",
            "@keyframes pulse{{0%,100%{{opacity:.55}}50%{{opacity:1}}}}",
            "h1{{margin:0 0 12px;font-size:30px;line-height:1.15;letter-spacing:-.02em}}",
            "p{{margin:0 0 14px;color:var(--muted)}}",
            ".code{{font:600 14px/1 ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;",
            "color:var(--muted);margin-top:28px;padding-top:20px;border-top:1px dashed var(--border);",
            "display:flex;justify-content:space-between;gap:12px;flex-wrap:wrap}}",
            ".actions{{display:flex;gap:10px;margin-top:24px;flex-wrap:wrap}}",
            ".btn{{appearance:none;border:1px solid var(--border);background:transparent;color:inherit;",
            "padding:10px 16px;border-radius:10px;font:inherit;font-weight:600;cursor:pointer;",
            "text-decoration:none;display:inline-flex;align-items:center;gap:8px}}",
            ".btn.primary{{background:var(--accent);color:#fff;border-color:transparent}}",
            ".btn:hover{{transform:translateY(-1px)}}",
            "</style></head><body>",
            "<main class=\"card\" role=\"alert\" aria-live=\"polite\">",
            "<p class=\"eyebrow\"><span class=\"dot\" aria-hidden=\"true\"></span>Error {code} &middot; {phrase}</p>",
            "<h1>{title}</h1><p>{summary}</p><p>{advice}</p>",
            "<div class=\"actions\">",
            "<a class=\"btn primary\" href=\"javascript:location.reload()\">Try again</a>",
            "<a class=\"btn\" href=\"/\">Go home</a>",
            "</div>",
            "<div class=\"code\"><span>nexide shield</span><span>HTTP {code}</span></div>",
            "</main></body></html>"
        ),
        code = code,
        phrase = phrase,
        title = html_escape(c.title),
        summary = html_escape(c.summary),
        advice = html_escape(c.advice),
        accent = c.accent,
    )
}

fn render_json(status: StatusCode, c: &Copy<'_>, detail: Option<&str>) -> String {
    let phrase = status.canonical_reason().unwrap_or("Error");
    let detail_segment = detail.map_or(String::new(), |d| {
        format!(",\"detail\":\"{}\"", json_escape(d))
    });
    format!(
        "{{\"error\":{{\"status\":{code},\"code\":\"{phrase}\",\"title\":\"{title}\",\"summary\":\"{summary}\",\"advice\":\"{advice}\"{detail}}}}}",
        code = status.as_u16(),
        phrase = json_escape(phrase),
        title = json_escape(c.title),
        summary = json_escape(c.summary),
        advice = json_escape(c.advice),
        detail = detail_segment,
    )
}

fn render_text(status: StatusCode, c: &Copy<'_>) -> String {
    let phrase = status.canonical_reason().unwrap_or("Error");
    format!(
        "{} {}\n\n{}\n{}\n{}\n",
        status.as_u16(),
        phrase,
        c.title,
        c.summary,
        c.advice
    )
}

fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                use std::fmt::Write as _;
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;

    async fn body_string(resp: Response<Body>) -> (StatusCode, String, String) {
        let status = resp.status();
        let ct = resp
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, ct, String::from_utf8_lossy(&bytes).into_owned())
    }

    fn headers_with_accept(value: &'static str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(ACCEPT, HeaderValue::from_static(value));
        h
    }

    #[tokio::test]
    async fn html_negotiation_returns_inline_page() {
        let h = headers_with_accept("text/html,application/xhtml+xml,*/*;q=0.8");
        let resp = render(StatusCode::INTERNAL_SERVER_ERROR, Some(&h), None);
        let (status, ct, body) = body_string(resp).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(ct.starts_with("text/html"));
        assert!(body.contains("<!doctype html>"));
        assert!(body.contains("Error 500"));
        assert!(body.contains("not your fault") || body.contains("isn&#39;t your fault"));
    }

    #[tokio::test]
    async fn json_negotiation_returns_envelope() {
        let h = headers_with_accept("application/json");
        let resp = render(
            StatusCode::BAD_GATEWAY,
            Some(&h),
            Some("upstream connection refused"),
        );
        let (status, ct, body) = body_string(resp).await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert!(ct.starts_with("application/json"));
        assert!(body.contains("\"status\":502"));
        assert!(body.contains("upstream connection refused"));
    }

    #[tokio::test]
    async fn unknown_accept_falls_back_to_text() {
        let h = headers_with_accept("application/octet-stream");
        let resp = render(StatusCode::SERVICE_UNAVAILABLE, Some(&h), None);
        let (_status, ct, body) = body_string(resp).await;
        assert!(ct.starts_with("text/plain"));
        assert!(body.starts_with("503 "));
    }

    #[tokio::test]
    async fn missing_accept_defaults_to_html() {
        let resp = render(StatusCode::NOT_FOUND, None, None);
        let (_status, ct, body) = body_string(resp).await;
        assert!(ct.starts_with("text/html"));
        assert!(body.contains("Error 404"));
    }

    #[test]
    fn html_escapes_user_visible_strings() {
        let escaped = html_escape("<script>x</script>");
        assert_eq!(escaped, "&lt;script&gt;x&lt;/script&gt;");
    }

    #[tokio::test]
    async fn json_escapes_detail() {
        let h = headers_with_accept("application/json");
        let resp = render(
            StatusCode::INTERNAL_SERVER_ERROR,
            Some(&h),
            Some("a \"quoted\"\n line"),
        );
        let (_status, _ct, body) = body_string(resp).await;
        assert!(body.contains("a \\\"quoted\\\"\\n line"));
    }

    #[tokio::test]
    async fn cache_control_is_no_store() {
        let resp = render(StatusCode::BAD_GATEWAY, None, None);
        assert_eq!(
            resp.headers()
                .get(CACHE_CONTROL)
                .and_then(|v| v.to_str().ok()),
            Some("no-store")
        );
    }
}
