//! `agents enqueue` + `agents notify`, tag flavor.
//!
//! Two `agents enqueue --agent-tag mock-handler` calls queue two
//! tweets, each stamped with the caller's
//! `deliverer_agent_instance_hierarchy` (the harness pins
//! `OBJECTIVEAI_AGENT_INSTANCE_HIERARCHY=test-harness` on the host,
//! which stamps it onto the plugin) and the free-text message.
//!
//! `agents notify` then reads the per-(agent, kind) counts and asks
//! the objectiveai host to `agents message --enqueue-with-key
//! psychological-operations` each agent. No agent is bound to the
//! tag, so the host enqueues the message against the tag name in
//! its per-test embedded postgres and answers
//! `{"type":"enqueued","id":1,"agent_tag":"mock-handler"}` — which
//! the plugin re-emits verbatim. Fully local: queue I/O is SQLite,
//! the message lands in the test-scoped postgres.

mod common;

use common::TestEnv;
use psychological_operations_sdk::x::queue::{AgentKind, Queue};

#[test]
fn agents_enqueue_and_notify() {
    let env = TestEnv::new("agents_enqueue_and_notify");

    // -- enqueue two tweets for the same tag agent ------------------
    for (tweet_id, message) in [("1900000000000000111", "first look"), ("1900000000000000222", "second look")] {
        let out = env.run(&[
            "agents", "enqueue",
            "--agent-tag", "mock-handler",
            "--tweet-id", tweet_id,
            "--message", message,
        ]);
        assert!(
            out.status.success(),
            "enqueue {tweet_id} failed: stderr={}",
            out.stderr,
        );
        common::snapshot::assert_snapshot(
            out.stdout_trimmed(),
            concat!(env!("CARGO_MANIFEST_DIR"), "/assets/agents_enqueue_and_notify/stdout_enqueue.txt"),
            include_str!("../assets/agents_enqueue_and_notify/stdout_enqueue.txt"),
        );
    }

    // -- the rows land verbatim in queue.sqlite ---------------------
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let rows = rt.block_on(async {
        let q = Queue::open(&env.base).await.expect("queue open");
        q.list("mock-handler").await.expect("list mock-handler")
    });
    assert_eq!(rows.len(), 2, "expected 2 queued tweets, got {rows:#?}");
    for (row, (tweet_id, message)) in rows.iter().zip([
        ("1900000000000000111", "first look"),
        ("1900000000000000222", "second look"),
    ]) {
        assert_eq!(row.agent, "mock-handler");
        assert_eq!(row.agent_kind, AgentKind::AgentTag);
        assert_eq!(row.tweet_id, tweet_id);
        assert_eq!(row.psyop, None, "operator rows carry no psyop");
        assert_eq!(row.score, None, "operator rows carry no score");
        assert_eq!(
            row.deliverer_agent_instance_hierarchy.as_deref(),
            Some("test-harness"),
            "deliverer = the caller's OBJECTIVEAI_AGENT_INSTANCE_HIERARCHY",
        );
        assert_eq!(row.message.as_deref(), Some(message));
    }

    // -- notify: one keyed `agents message` per queued agent --------
    let out = env.run(&["agents", "notify"]);
    assert!(
        out.status.success(),
        "notify failed: stderr={}",
        out.stderr,
    );
    common::snapshot::assert_snapshot(
        out.stdout_trimmed(),
        concat!(env!("CARGO_MANIFEST_DIR"), "/assets/agents_enqueue_and_notify/stdout_notify.txt"),
        include_str!("../assets/agents_enqueue_and_notify/stdout_notify.txt"),
    );
    common::snapshot::assert_snapshot(
        out.stderr_trimmed(),
        concat!(env!("CARGO_MANIFEST_DIR"), "/assets/agents_enqueue_and_notify/stderr_notify.txt"),
        include_str!("../assets/agents_enqueue_and_notify/stderr_notify.txt"),
    );
}
