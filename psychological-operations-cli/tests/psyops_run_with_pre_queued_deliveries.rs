//! `data.db` is pre-seeded with a single `delivery_queue` row for
//! "test-psyop" (target=stdout-urls, two stub post ids). The
//! psyop's `config.json` declares no targets, so the run's step-10
//! enqueue is a no-op — but step 11 still drains, picking up the
//! pre-queued row alongside zero new ones.

mod common;

use common::TestEnv;

#[test]
fn psyops_run_with_pre_queued_deliveries() {
    let env = TestEnv::new("psyops_run_with_pre_queued_deliveries");

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
        concat!(env!("CARGO_MANIFEST_DIR"), "/assets/psyops_run_with_pre_queued_deliveries/stdout.txt"),
        include_str!("../assets/psyops_run_with_pre_queued_deliveries/stdout.txt"),
    );
    common::snapshot::assert_snapshot(
        &common::snapshot::normalize(out.stderr_trimmed()),
        concat!(env!("CARGO_MANIFEST_DIR"), "/assets/psyops_run_with_pre_queued_deliveries/stderr.txt"),
        include_str!("../assets/psyops_run_with_pre_queued_deliveries/stderr.txt"),
    );
}
