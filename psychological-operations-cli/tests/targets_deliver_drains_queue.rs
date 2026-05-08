//! Two pre-queued delivery_queue rows for "test-psyop"
//! (stdout-urls + stdout-json). `targets deliver` should drain
//! both and report `{ pending: 2, delivered: 2, failed: 0 }`.

mod common;

use common::TestEnv;

#[test]
fn targets_deliver_drains_queue() {
    let env = TestEnv::new("targets_deliver_drains_queue");

    let out = env.run(&["targets", "deliver"]);
    assert!(
        out.status.success(),
        "deliver failed: stderr={}",
        out.stderr,
    );

    common::snapshot::assert_snapshot(
        &common::snapshot::normalize(out.stdout_trimmed()),
        concat!(env!("CARGO_MANIFEST_DIR"), "/assets/targets_deliver_drains_queue/stdout.txt"),
        include_str!("../assets/targets_deliver_drains_queue/stdout.txt"),
    );
    common::snapshot::assert_snapshot(
        &common::snapshot::normalize(out.stderr_trimmed()),
        concat!(env!("CARGO_MANIFEST_DIR"), "/assets/targets_deliver_drains_queue/stderr.txt"),
        include_str!("../assets/targets_deliver_drains_queue/stderr.txt"),
    );
}
