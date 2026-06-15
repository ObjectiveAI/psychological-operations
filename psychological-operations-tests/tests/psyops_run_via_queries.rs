//! `psyops run` ingests via a mock-X search query and runs end-to-end
//! (no scoring stages → max-score survivors). (Old: psyops_run_query_fallback
//! — minus the pre-seeded for_you queue, which forced the fallback.)

use psychological_operations_tests::{Plugin, query_psyop};

#[tokio::test]
async fn psyops_run_via_queries() {
    let p = Plugin::new("psyops_run_via_queries");
    p.psyops_publish("test-psyop", &query_psyop("mock fallback search", vec![]))
        .await
        .assert_no_errors();

    let run = p.psyops_run(&["test-psyop"], Some(42)).await;
    run.assert_no_errors();
    assert!(
        run.has_event("query_complete"),
        "expected a query_complete event, got {:?}",
        run.events,
    );
}
