//! Pre-seeded `data.db` has 2 rows in `for_you_queue` for psyop
//! "test-psyop" referencing the deterministic commit_sha that
//! the harness's git-init produces. The asset folder also ships
//! the `psyops/test-psyop/psyop.json` (no .git — harness
//! initializes it on copy).
//!
//! Test runs `psyops run --name test-psyop --seed 42`. The
//! runtime hydrates the queue via mock-X /tweets/{id}, scores
//! the tweets, persists, and reaps contents.

mod common;

use common::TestEnv;

#[test]
fn psyops_run_with_for_you_queue() {
    let env = TestEnv::new("psyops_run_with_for_you_queue");

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
        concat!(env!("CARGO_MANIFEST_DIR"), "/assets/psyops_run_with_for_you_queue/stdout.txt"),
        include_str!("../assets/psyops_run_with_for_you_queue/stdout.txt"),
    );
    common::snapshot::assert_snapshot(
        &common::snapshot::normalize(out.stderr_trimmed()),
        concat!(env!("CARGO_MANIFEST_DIR"), "/assets/psyops_run_with_for_you_queue/stderr.txt"),
        include_str!("../assets/psyops_run_with_for_you_queue/stderr.txt"),
    );
}
