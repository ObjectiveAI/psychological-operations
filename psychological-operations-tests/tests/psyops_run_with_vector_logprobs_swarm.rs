//! A single scoring stage backed by a vector-ranking logprobs swarm
//! runs end-to-end. (Old: psyops_run_with_vector_logprobs_swarm.)

use psychological_operations_tests::{Plugin, mock_function_stage, query_psyop};

#[tokio::test]
async fn psyops_run_with_vector_logprobs_swarm() {
    let p = Plugin::new("psyops_run_with_vector_logprobs_swarm");
    let stages = vec![mock_function_stage("tweet-ranker", "high-logprobs-duo")];
    p.psyops_publish("test-psyop", &query_psyop("tweet ranker vector swarm", stages))
        .await
        .assert_no_errors();

    let run = p.psyops_run(&["test-psyop"], Some(42)).await;
    run.assert_no_errors();
    assert_eq!(
        run.event_count("stage_begin"),
        1,
        "the single vector scoring stage should run, got {:?}",
        run.events,
    );
}
