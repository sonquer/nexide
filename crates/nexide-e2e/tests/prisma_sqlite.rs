//! E2E test for the Prisma + SQLite fixture under `e2e/prisma-sqlite`.
//!
//! Validates that nexide can serve a real Next.js standalone bundle
//! whose request path goes:
//!
//!     route handler → @prisma/client → libquery_engine.node (N-API) → SQLite
//!
//! Gated behind `#[ignore]` because it requires an external build:
//!
//! ```text
//! ( cd e2e/prisma-sqlite && pnpm install && pnpm build )
//! cargo build --release
//! cargo test -p nexide-e2e --release prisma_sqlite -- --ignored --nocapture
//! ```

use std::time::Duration;

use nexide_e2e::{
    NexideProcess, prisma_prerequisites_present, prisma_sqlite_standalone, workspace_root,
};

const READY_TIMEOUT: Duration = Duration::from_secs(45);

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires built e2e/prisma-sqlite fixture and release nexide"]
async fn prisma_sqlite_serves_seeded_users() {
    if !prisma_prerequisites_present() {
        panic!(
            "missing fixture build at {} — run `cd e2e/prisma-sqlite && pnpm install && pnpm build`",
            prisma_sqlite_standalone().display()
        );
    }

    let cwd = workspace_root().join("e2e/prisma-sqlite");
    let server = NexideProcess::spawn_at(cwd, READY_TIMEOUT)
        .await
        .expect("nexide ready against prisma-sqlite fixture");

    let base = format!("http://{}", server.addr());
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();

    let resp = client
        .get(format!("{base}/api/users"))
        .send()
        .await
        .expect("GET /api/users");
    assert_eq!(resp.status().as_u16(), 200, "/api/users status");
    let body: serde_json::Value = resp.json().await.expect("json body");

    assert_eq!(body["runtime"], "nexide");
    assert_eq!(body["engine"], "prisma-library");
    assert_eq!(body["count"], 2, "expected 2 seeded users, got {body}");
    let users = body["users"].as_array().expect("users array");
    assert_eq!(users.len(), 2);

    let alice = users
        .iter()
        .find(|u| u["email"] == "alice@example.com")
        .expect("alice present");
    assert_eq!(alice["posts"], 2);
    let bob = users
        .iter()
        .find(|u| u["email"] == "bob@example.com")
        .expect("bob present");
    assert_eq!(bob["posts"], 1);

    let html_resp = client.get(&base).send().await.expect("GET / for SSR HTML");
    assert_eq!(html_resp.status().as_u16(), 200, "/ status");
    let html = html_resp.text().await.expect("html body");
    assert!(
        html.contains("Prisma users"),
        "SSR HTML missing page marker: {html}"
    );
    assert!(html.contains("Alice"), "SSR HTML missing Alice: {html}");
    assert!(html.contains("Bob"), "SSR HTML missing Bob: {html}");
    assert!(
        html.contains("count="),
        "SSR HTML missing user-count: {html}"
    );
}
