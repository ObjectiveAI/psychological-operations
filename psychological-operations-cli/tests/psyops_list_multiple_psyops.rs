//! Asset folder pre-loads 3 psyop dirs (`a-psyop`, `b-psyop`,
//! `c-psyop`) under `.psychological-operations/psyops/`. The
//! harness git-inits each one at copy time. Test runs
//! `psyops list` and snapshots the output.

mod common;

use common::TestEnv;

#[test]
fn psyops_list_multiple_psyops() {
    let env = TestEnv::new("psyops_list_multiple_psyops");

    let list = env.run(&["psyops", "list"]);
    assert!(
        list.status.success(),
        "psyops list failed: stderr={}",
        list.stderr,
    );

    common::snapshot::assert_snapshot(
        list.stdout_trimmed(),
        concat!(env!("CARGO_MANIFEST_DIR"), "/assets/psyops_list_multiple_psyops/stdout.txt"),
        include_str!("../assets/psyops_list_multiple_psyops/stdout.txt"),
    );
    common::snapshot::assert_snapshot(
        list.stderr_trimmed(),
        concat!(env!("CARGO_MANIFEST_DIR"), "/assets/psyops_list_multiple_psyops/stderr.txt"),
        include_str!("../assets/psyops_list_multiple_psyops/stderr.txt"),
    );
}
