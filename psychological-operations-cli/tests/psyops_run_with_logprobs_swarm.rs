//! Same shape as psyops_run_query_fallback, except the stage's
//! profile is `mock/high-logprobs-duo` (two mock agents with
//! top_logprobs: 15) instead of `mock/solo-instruction` (single
//! agent, no logprobs). Diagnostic: the snapshot tells us whether
//! logprobs voting actually varies the per-post scores in mock
//! mode.

mod common;

use common::TestEnv;

#[test]
fn psyops_run_with_logprobs_swarm() {
    let env = TestEnv::new("psyops_run_with_logprobs_swarm");

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
        concat!(env!("CARGO_MANIFEST_DIR"), "/assets/psyops_run_with_logprobs_swarm/stdout.txt"),
        include_str!("../assets/psyops_run_with_logprobs_swarm/stdout.txt"),
    );
    common::snapshot::assert_snapshot(
        &common::snapshot::normalize(out.stderr_trimmed()),
        concat!(env!("CARGO_MANIFEST_DIR"), "/assets/psyops_run_with_logprobs_swarm/stderr.txt"),
        include_str!("../assets/psyops_run_with_logprobs_swarm/stderr.txt"),
    );
}
