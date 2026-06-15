//! A single scoring stage backed by a logprobs swarm profile runs
//! end-to-end. (Old: psyops_run_with_logprobs_swarm.)

use psychological_operations_tests::{Plugin, mock_function_stage, query_psyop};

#[tokio::test]
async fn psyops_run_with_logprobs_swarm() {
    let p = Plugin::new("psyops_run_with_logprobs_swarm");
    let stages = vec![mock_function_stage(Some(15))];
    p.psyops_publish("test-psyop", &query_psyop("mock fallback search", stages))
        .await
        .assert_no_errors();

    let run = p.psyops_run(&["test-psyop"], Some(42)).await;
    run.assert_no_errors();
    assert_eq!(
        run.event_count("stage_begin"),
        1,
        "the single scoring stage should run, got {:?}",
        run.events,
    );
}
