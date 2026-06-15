//! Publish a psyop from a typed definition, then confirm it appears in
//! `psyops list` — everything through the executor, no filesystem reads.

use psychological_operations_sdk::cli::psyops::{ForYou, PsyOp, SortBy};
use psychological_operations_tests::Plugin;

#[tokio::test]
async fn psyops_publish_and_list() {
    let p = Plugin::new("psyops_publish_and_list");

    // Typed definition → compile-time-checked; the harness serializes it
    // to the `--psyop-inline` JSON.
    let psyop = PsyOp {
        queries: None,
        for_you: Some(ForYou { priority: None, filter: None }),
        interval: "1h".into(),
        min_posts: 2,
        max_posts: 10,
        sort: SortBy::Newest,
        query_when_for_you_queued: true,
        stages: None,
    };

    let published = p.psyops_publish("test-psyop", &psyop).await;
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
