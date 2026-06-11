//! `agents notify`, agent-instance-hierarchy flavor.
//!
//! `agents enqueue --agent-instance mock-instance` (no explicit
//! parent) composes the stored agent as
//! `<caller hierarchy>/mock-instance` = `test-harness/mock-instance`
//! and records `agent_kind = agent_instance_hierarchy`. `agents
//! notify` must then split that hierarchy at the last '/' into a
//! Direct message target — parent `test-harness`, instance
//! `mock-instance` — so the host re-composes the exact same
//! hierarchy when it enqueues the keyed message, answering
//! `{"type":"enqueued","id":1,"agent_instance_hierarchy":
//! "test-harness/mock-instance"}`.

mod common;

use common::TestEnv;
use psychological_operations_sdk::x::queue::{AgentKind, Queue};

#[test]
fn agents_notify_direct() {
    let env = TestEnv::new("agents_notify_direct");

    let out = env.run(&[
        "agents", "enqueue",
        "--agent-instance", "mock-instance",
        "--tweet-id", "1900000000000000333",
        "--message", "direct delivery",
    ]);
    assert!(
        out.status.success(),
        "enqueue failed: stderr={}",
        out.stderr,
    );

    // The composed hierarchy landed verbatim (slashes preserved).
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let rows = rt.block_on(async {
        let q = Queue::open(&env.base).await.expect("queue open");
        q.list("test-harness/mock-instance").await.expect("list hierarchy agent")
    });
    assert_eq!(rows.len(), 1, "expected 1 queued tweet, got {rows:#?}");
    assert_eq!(rows[0].agent_kind, AgentKind::AgentInstanceHierarchy);
    assert_eq!(rows[0].tweet_id, "1900000000000000333");
    assert_eq!(
        rows[0].deliverer_agent_instance_hierarchy.as_deref(),
        Some("test-harness"),
    );

    let out = env.run(&["agents", "notify"]);
    assert!(
        out.status.success(),
        "notify failed: stderr={}",
        out.stderr,
    );
    common::snapshot::assert_snapshot(
        out.stdout_trimmed(),
        concat!(env!("CARGO_MANIFEST_DIR"), "/assets/agents_notify_direct/stdout_notify.txt"),
        include_str!("../assets/agents_notify_direct/stdout_notify.txt"),
    );
    common::snapshot::assert_snapshot(
        out.stderr_trimmed(),
        concat!(env!("CARGO_MANIFEST_DIR"), "/assets/agents_notify_direct/stderr_notify.txt"),
        include_str!("../assets/agents_notify_direct/stderr_notify.txt"),
    );
}
