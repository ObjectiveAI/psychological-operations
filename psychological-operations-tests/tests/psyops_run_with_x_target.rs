//! Add a global X "like" target, run a psyop, and confirm the like
//! delivers (mock-X — no real network or auth). (Old: psyops_run_with_x_target.)
//!
//! The X destination emits no `target_delivered` event on success (only a
//! `delivery_failed` error frame on failure), and `psyops run` discards the
//! delivery summary. So the proof is two-sided: `query_complete` fires ⇒ at
//! least one post was ingested ⇒ with no scoring stages every post is a
//! survivor ⇒ the X like is attempted; and `assert_no_errors()` ⇒ the mock
//! like succeeded (no `delivery_failed`).

use psychological_operations_sdk::cli::destinations::Destination;
use psychological_operations_sdk::cli::destinations::x::{X, XType};
use psychological_operations_tests::{Plugin, Selector, query_psyop};

#[tokio::test]
async fn psyops_run_with_x_target() {
    let p = Plugin::new("psyops_run_with_x_target");
    p.targets_add(Selector::Global, &Destination::X(X { r#type: XType::Like }))
        .await
        .assert_ok();
    p.psyops_publish("test-psyop", &query_psyop("mock fallback search", vec![]))
        .await
        .assert_no_errors();

    let run = p.psyops_run(&["test-psyop"], Some(42)).await;
    run.assert_no_errors();
    assert!(
        run.has_event("query_complete"),
        "expected a query_complete event (⇒ a survivor ⇒ an X like attempt), got {:?}",
        run.events,
    );
}
