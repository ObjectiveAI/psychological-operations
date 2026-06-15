//! Add a global stdout target, run a psyop, and confirm a delivery
//! fires. (Old: psyops_run_with_stdout_target.)

use psychological_operations_sdk::cli::destinations::Destination;
use psychological_operations_sdk::cli::destinations::stdout::Stdout;
use psychological_operations_tests::{Plugin, Selector, query_psyop};

#[tokio::test]
async fn psyops_run_with_stdout_target() {
    let p = Plugin::new("psyops_run_with_stdout_target");
    p.targets_add(Selector::Global, &Destination::Stdout(Stdout::default()))
        .await
        .assert_ok();
    p.psyops_publish("test-psyop", &query_psyop("mock fallback search", vec![]))
        .await
        .assert_no_errors();

    let run = p.psyops_run(&["test-psyop"], Some(42)).await;
    run.assert_no_errors();
    assert!(
        run.has_event("target_delivered"),
        "expected a target_delivered event, got {:?}",
        run.events,
    );
}
