//! Integration smoke test for the `nexide` runtime entrypoint.

use std::time::Duration;

use nexide::serve_until;
use tokio::sync::oneshot;
use tokio::time::timeout;

/// End-to-end shutdown smoke. Requires either a built Next.js
/// standalone bundle or the canary `nexide_app.mjs`; ignored by
/// default because real Next.js boot is a heavyweight integration
/// scenario kept out of the default test run.
///
/// Run explicitly with:
/// `cargo test -p nexide-e2e -- --ignored runtime_smoke`.
#[tokio::test]
#[ignore = "exercises full runtime boot — gated behind --ignored"]
async fn shuts_down_when_external_signal_resolves() {
    let (tx, rx) = oneshot::channel::<()>();

    let handle = tokio::spawn(async move {
        serve_until(async move {
            let _ = rx.await;
        })
        .await
    });

    tokio::time::sleep(Duration::from_millis(20)).await;
    tx.send(()).expect("receiver alive");

    let join = timeout(Duration::from_secs(2), handle)
        .await
        .expect("runtime did not shut down in time")
        .expect("task did not panic");

    join.expect("runtime returned an error");
}
