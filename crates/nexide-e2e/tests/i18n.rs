//! E2E test for the i18n fixture under `e2e/i18n`.
//!
//! Validates `i18next` + `i18next-resources-to-backend` +
//! `next-i18n-router` plus the V8 ICU and TextDecoder code paths
//! when running under nexide.  Boots the runtime with empty
//! `LANG`/`LC_ALL` to mirror the slim-container failure mode.
//!
//! ```text
//! ( cd e2e/i18n && pnpm install && pnpm build )
//! cargo build --release
//! cargo test -p nexide-e2e --release i18n -- --ignored --nocapture
//! ```

use std::time::Duration;

use nexide_e2e::{NexideProcess, i18n_prerequisites_present, i18n_standalone, workspace_root};

const READY_TIMEOUT: Duration = Duration::from_secs(45);

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires built e2e/i18n fixture and release nexide"]
async fn i18n_serves_translated_pages() {
    if !i18n_prerequisites_present() {
        panic!(
            "missing fixture build at {} — run `cd e2e/i18n && pnpm install && pnpm build`",
            i18n_standalone().display()
        );
    }

    let cwd = workspace_root().join("e2e/i18n");
    let server =
        NexideProcess::spawn_at_with_env(cwd, READY_TIMEOUT, &[("LANG", ""), ("LC_ALL", "")])
            .await
            .expect("nexide ready against i18n fixture");

    let base = format!("http://{}", server.addr());
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    let ping = client
        .get(format!("{base}/api/ping"))
        .send()
        .await
        .expect("GET /api/ping");
    assert_eq!(ping.status().as_u16(), 200, "/api/ping status");

    let fmt = client
        .get(format!("{base}/api/format"))
        .send()
        .await
        .expect("GET /api/format");
    assert_eq!(fmt.status().as_u16(), 200, "/api/format status");
    let body: serde_json::Value = fmt.json().await.expect("/api/format json");
    assert_eq!(body["runtime"], "nexide");
    assert!(
        body["noArgs"].as_str().is_some_and(|s| !s.is_empty()),
        "noArgs toLocaleString must succeed: {body}"
    );
    assert!(
        body["explicit"]["pl-PL"]
            .as_str()
            .is_some_and(|s| s.contains('\u{a0}') || s.contains(' ')),
        "pl-PL number formatted: {body}"
    );
    assert_eq!(
        body["polish"], "Zażółć gęślą jaźń — pchnąć w tę łódź jeża",
        "Polish UTF-8 round-trip: {body}"
    );

    let root = client.get(&base).send().await.expect("GET /");
    assert!(
        matches!(root.status().as_u16(), 200 | 301 | 302 | 307 | 308),
        "/ should redirect or render: status={}",
        root.status()
    );

    for (path, marker) in [
        ("/", "Hello, świat"),
        ("/pl", "Cześć, świat"),
        ("/pl/utf8", "Zażółć gęślą jaźń"),
        ("/utf8", "Polish text test"),
        ("/pl/static", "Zażółć gęślą jaźń"),
        ("/en/static", "Polish text test"),
    ] {
        let resp = client
            .get(format!("{base}{path}"))
            .send()
            .await
            .unwrap_or_else(|e| panic!("GET {path} failed: {e}"));
        assert_eq!(resp.status().as_u16(), 200, "{path} status");
        let html = resp.text().await.expect("html body");
        assert!(
            html.contains(marker),
            "{path} missing marker '{marker}': {}",
            &html[..html.len().min(800)]
        );
    }

    for (path, expected_loc) in [("/en", "/"), ("/en/utf8", "/utf8")] {
        let resp = client
            .get(format!("{base}{path}"))
            .send()
            .await
            .unwrap_or_else(|e| panic!("GET {path} failed: {e}"));
        assert_eq!(resp.status().as_u16(), 307, "{path} status");
        let loc = resp
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            loc.starts_with(expected_loc),
            "{path} location='{loc}' expected to start with '{expected_loc}'"
        );
    }

    let intl = client
        .get(format!("{base}/pl/intl"))
        .send()
        .await
        .expect("GET /pl/intl");
    assert_eq!(intl.status().as_u16(), 200, "/pl/intl status");
    let html = intl.text().await.expect("intl body");
    assert!(
        html.contains("default="),
        "intl page rendered without ICU error: {}",
        &html[..html.len().min(800)]
    );
}
