//! Two psyops on disk; `psyops run --name target-psyop` should
//! exercise only the named one — sibling-psyop's query should
//! never be sent.

mod common;

use common::TestEnv;

#[test]
fn psyops_run_one_of_multiple() {
    let env = TestEnv::new("psyops_run_one_of_multiple");

    let out = env.run(&[
        "psyops", "run",
        "--name", "target-psyop",
        "--seed", "42",
    ]);
    assert!(
        out.status.success(),
        "run failed: stderr={}",
        out.stderr,
    );

    common::snapshot::assert_snapshot(
        &common::snapshot::normalize(out.stdout_trimmed()),
        concat!(env!("CARGO_MANIFEST_DIR"), "/assets/psyops_run_one_of_multiple/stdout.txt"),
        include_str!("../assets/psyops_run_one_of_multiple/stdout.txt"),
    );
    common::snapshot::assert_snapshot(
        &common::snapshot::normalize(out.stderr_trimmed()),
        concat!(env!("CARGO_MANIFEST_DIR"), "/assets/psyops_run_one_of_multiple/stderr.txt"),
        include_str!("../assets/psyops_run_one_of_multiple/stderr.txt"),
    );
}
