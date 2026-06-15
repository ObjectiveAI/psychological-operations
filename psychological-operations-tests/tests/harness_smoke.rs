//! Smoke test: a fresh isolated state has no psyops, and the
//! executor + two-layer deserialize round-trips cleanly.

use psychological_operations_tests::Plugin;

#[tokio::test]
async fn harness_smoke() {
    let p = Plugin::new("harness_smoke");
    let r = p.psyops_list().await;
    r.assert_no_errors();
    assert!(
        r.psyop_list().is_empty(),
        "fresh state should have no psyops, got {:?}",
        r.psyop_list(),
    );
}
