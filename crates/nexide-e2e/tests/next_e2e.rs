//! End-to-end smoke against a real Next.js standalone build.
//!
//! Mirrors the validation matrix that used to live in
//! `scripts/e2e_next.sh`, but expressed as native Rust so it ships
//! with the workspace and runs under `cargo test`.
//!
//! Gated behind `#[ignore]` because it requires:
//!   * `npm run build` to have produced `example/.next/standalone/server.js`
//!   * `cargo build --release` to have produced `target/release/nexide`
//!
//! Run explicitly:
//! ```text
//! cargo test -p nexide-e2e --release -- --ignored --test-threads=1
//! ```

use std::time::Duration;

use nexide_e2e::NexideProcess;

const READY_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug)]
struct ProbeOutcome {
    status: u16,
    body: String,
    content_type: String,
}

async fn probe(client: &reqwest::Client, url: &str) -> anyhow::Result<ProbeOutcome> {
    let resp = client.get(url).send().await?;
    let status = resp.status().as_u16();
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let body = resp.text().await?;
    Ok(ProbeOutcome {
        status,
        body,
        content_type,
    })
}

type ProbeCase = (
    &'static str,
    &'static str,
    u16,
    Option<&'static str>,
    Option<&'static str>,
);

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires `next build` in example and `cargo build --release`"]
async fn next_e2e_smoke_runs_against_standalone_build() {
    let server = NexideProcess::spawn(READY_TIMEOUT)
        .await
        .expect("nexide ready");
    let base = format!("http://{}", server.addr());
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();

    let cases: &[ProbeCase] = &[
        (
            "ssr index",
            "/",
            200,
            Some("data-testid=\"ssr-marker\""),
            None,
        ),
        (
            "api ping",
            "/api/ping",
            200,
            Some("\"runtime\":\"nexide\""),
            None,
        ),
        ("user dynamic", "/users/123", 200, Some("123"), None),
        ("public asset", "/logo.svg", 200, None, Some("image/svg")),
    ];

    for (label, path, expected_status, body_needle, ct_needle) in cases {
        let url = format!("{base}{path}");
        let outcome = probe(&client, &url).await.expect("probe");
        assert_eq!(
            outcome.status, *expected_status,
            "{label}: expected {expected_status}, got {}",
            outcome.status
        );
        if let Some(needle) = body_needle {
            assert!(
                outcome.body.contains(needle),
                "{label}: body missing '{needle}' (got: {})",
                outcome.body
            );
        }
        if let Some(needle) = ct_needle {
            assert!(
                outcome.content_type.contains(needle),
                "{label}: content-type missing '{needle}' (got: {})",
                outcome.content_type
            );
        }
    }
}

#[test]
fn entrypoint_resolver_prefers_standalone_when_present() {
    use nexide::entrypoint::{EntrypointKind, EntrypointResolver, ProductionEntrypointResolver};
    use std::fs;

    let dir = tempfile::tempdir().expect("tmp");
    fs::create_dir_all(dir.path().join(".next/standalone")).expect("mkdirs");
    fs::write(
        dir.path().join(".next/standalone/server.js"),
        "// next standalone",
    )
    .expect("write standalone");

    let resolved = ProductionEntrypointResolver::new(dir.path())
        .resolve()
        .expect("resolved");
    assert_eq!(resolved.kind, EntrypointKind::NextStandalone);
}
