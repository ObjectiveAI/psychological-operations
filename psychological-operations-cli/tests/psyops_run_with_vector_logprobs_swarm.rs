//! Vector-function counterpart to psyops_run_with_logprobs_swarm.
//! Function is `mock/tweet-ranker` (alpha.vector.branch.function —
//! takes `{items: [...]}` and returns one score per item) paired
//! with `mock/high-logprobs-duo` (two mock agents, top_logprobs: 15).
//! Diagnostic: confirms vector-function logprobs voting also
//! produces varied scores and exits cleanly through our pipeline.

mod common;

use common::TestEnv;

#[test]
fn psyops_run_with_vector_logprobs_swarm() {
    let env = TestEnv::new("psyops_run_with_vector_logprobs_swarm");

    let out = env.run(&[
        "psyops", "run",
        "--name", "test-psyop",
        "--seed", "42",
    ]);
    assert!(
        out.status.success(),
        "run failed: stderr={}",
        out.stderr,
    );

    common::snapshot::assert_snapshot(
        &common::snapshot::normalize(out.stdout_trimmed()),
        concat!(env!("CARGO_MANIFEST_DIR"), "/assets/psyops_run_with_vector_logprobs_swarm/stdout.txt"),
        include_str!("../assets/psyops_run_with_vector_logprobs_swarm/stdout.txt"),
    );
    common::snapshot::assert_snapshot(
        &common::snapshot::normalize(out.stderr_trimmed()),
        concat!(env!("CARGO_MANIFEST_DIR"), "/assets/psyops_run_with_vector_logprobs_swarm/stderr.txt"),
        include_str!("../assets/psyops_run_with_vector_logprobs_swarm/stderr.txt"),
    );
}
