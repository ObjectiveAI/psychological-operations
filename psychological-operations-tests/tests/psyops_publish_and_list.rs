//! Publish a psyop from a typed definition, then confirm it appears in
//! `psyops list` — everything through the executor, no filesystem reads.

use psychological_operations_tests::{Plugin, query_psyop};

#[tokio::test]
async fn psyops_publish_and_list() {
    let p = Plugin::new("psyops_publish_and_list");

    // Typed definition (no `for_you`, ingests via a mock-X query) →
    // compile-time-checked; the harness serializes it to `--psyop-inline`.
    let published = p
        .psyops_publish("test-psyop", &query_psyop("mock fallback search", vec![]))
        .await;
    published.assert_no_errors();
    assert_eq!(published.published().name, "test-psyop");

    let listed = p.psyops_list().await;
    listed.assert_no_errors();
    assert!(
        listed.psyop_list().iter().any(|e| e.name == "test-psyop"),
        "published psyop should be listed, got {:?}",
        listed.psyop_list(),
    );
}
