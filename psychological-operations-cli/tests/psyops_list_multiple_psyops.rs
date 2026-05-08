//! Publishes 3 psyops (a, b, c) then runs `psyops list`.
//! Snapshots the list output.

mod common;

use common::TestEnv;

#[test]
fn psyops_list_multiple_psyops() {
    let env = TestEnv::new("psyops_list_multiple_psyops");

    let psyop_file = env.assets.join("psyop.json");
    let psyop_arg = psyop_file.to_str().unwrap();

    for name in ["a-psyop", "b-psyop", "c-psyop"] {
        let out = env.run(&[
            "psyops", "publish",
            "--name", name,
            "--psyop-file", psyop_arg,
            "--message", "init",
        ]);
        assert!(
            out.status.success(),
            "publish {name} failed: stderr={}",
            out.stderr,
        );
    }

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
