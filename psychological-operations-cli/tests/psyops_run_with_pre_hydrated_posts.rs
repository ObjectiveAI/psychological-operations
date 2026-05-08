//! `data.db` is pre-seeded with `posts` + `contents` + `sources`
//! rows for psyop "test-psyop" — already hydrated, no
//! `for_you_queue` entries. Harness git-inits the
//! `psyops/test-psyop/` dir.
//!
//! `psyops run` should skip the X `/2/tweets/{id}` hydration call
//! entirely and go straight to filter → score against the
//! pre-existing posts.

mod common;

use common::TestEnv;

#[test]
fn psyops_run_with_pre_hydrated_posts() {
    let env = TestEnv::new("psyops_run_with_pre_hydrated_posts");

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
        concat!(env!("CARGO_MANIFEST_DIR"), "/assets/psyops_run_with_pre_hydrated_posts/stdout.txt"),
        include_str!("../assets/psyops_run_with_pre_hydrated_posts/stdout.txt"),
    );
    common::snapshot::assert_snapshot(
        &common::snapshot::normalize(out.stderr_trimmed()),
        concat!(env!("CARGO_MANIFEST_DIR"), "/assets/psyops_run_with_pre_hydrated_posts/stderr.txt"),
        include_str!("../assets/psyops_run_with_pre_hydrated_posts/stderr.txt"),
    );
}
