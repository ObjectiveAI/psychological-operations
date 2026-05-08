//! Empty `for_you_queue` + a `queries` entry → the run loop should
//! drop into the X v2 search-recent fallback after the first
//! filter pass falls below `min_posts`. Mock-X returns
//! deterministic tweets keyed on the query string, so the resulting
//! score output is byte-stable.

mod common;

use common::TestEnv;

#[test]
fn psyops_run_query_fallback() {
    let env = TestEnv::new("psyops_run_query_fallback");

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
        concat!(env!("CARGO_MANIFEST_DIR"), "/assets/psyops_run_query_fallback/stdout.txt"),
        include_str!("../assets/psyops_run_query_fallback/stdout.txt"),
    );
    common::snapshot::assert_snapshot(
        &common::snapshot::normalize(out.stderr_trimmed()),
        concat!(env!("CARGO_MANIFEST_DIR"), "/assets/psyops_run_query_fallback/stderr.txt"),
        include_str!("../assets/psyops_run_query_fallback/stderr.txt"),
    );
}
