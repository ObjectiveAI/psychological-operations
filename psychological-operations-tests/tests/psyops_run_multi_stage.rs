//! A two-stage scoring pipeline (both stages → the same mock function +
//! profile) runs end-to-end through the host. (Old: psyops_run_multi_stage.)

use psychological_operations_tests::{Plugin, mock_function_stage, query_psyop};

#[tokio::test]
async fn psyops_run_multi_stage() {
    let p = Plugin::new("psyops_run_multi_stage");
    let stages = vec![mock_function_stage(None), mock_function_stage(None)];
    p.psyops_publish("test-psyop", &query_psyop("mock fallback search", stages))
        .await
        .assert_no_errors();

    let run = p.psyops_run(&["test-psyop"], Some(42)).await;
    run.assert_no_errors();
    assert_eq!(
        run.event_count("stage_begin"),
        2,
        "both stages should run, got {:?}",
        run.events,
    );
}
