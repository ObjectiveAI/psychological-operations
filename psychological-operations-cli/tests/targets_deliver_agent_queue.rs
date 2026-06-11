//! Target delivery with mock agents. Two pre-queued delivery_queue
//! rows for "test-psyop" carry the two `agent_queue` destination
//! flavors — `{"agent_tag":"mock-agent"}` and
//! `{"agent_instance_hierarchy":"ops/mock-agent-2"}` — each listing
//! the same two scored posts. `targets deliver --global` must drain
//! both rows into the SDK's per-agent queue (queue.sqlite): one row
//! per (agent, tweet), carrying the psyop name + persisted score,
//! and NO deliverer/message (those columns belong to
//! `agents enqueue` rows). No agent has to exist anywhere — the
//! queue is the mailbox.

mod common;

use common::TestEnv;
use psychological_operations_sdk::x::queue::{AgentKind, Queue, QueueEntry};

#[test]
fn targets_deliver_agent_queue() {
    let env = TestEnv::new("targets_deliver_agent_queue");

    let out = env.run(&["targets", "deliver", "--global"]);
    assert!(
        out.status.success(),
        "deliver failed: stderr={}",
        out.stderr,
    );

    common::snapshot::assert_snapshot(
        out.stdout_trimmed(),
        concat!(env!("CARGO_MANIFEST_DIR"), "/assets/targets_deliver_agent_queue/stdout.txt"),
        include_str!("../assets/targets_deliver_agent_queue/stdout.txt"),
    );
    common::snapshot::assert_snapshot(
        out.stderr_trimmed(),
        concat!(env!("CARGO_MANIFEST_DIR"), "/assets/targets_deliver_agent_queue/stderr.txt"),
        include_str!("../assets/targets_deliver_agent_queue/stderr.txt"),
    );

    // The delivery's real product: queue.sqlite rows per agent.
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let (tag_rows, hier_rows) = rt.block_on(async {
        let q = Queue::open(&env.base).await.expect("queue open");
        (
            q.list("mock-agent").await.expect("list mock-agent"),
            q.list("ops/mock-agent-2").await.expect("list ops/mock-agent-2"),
        )
    });

    assert_queue_rows(&tag_rows, "mock-agent", AgentKind::AgentTag);
    assert_queue_rows(&hier_rows, "ops/mock-agent-2", AgentKind::AgentInstanceHierarchy);
}

/// Both agents receive identical deliveries: the two seeded posts
/// with their persisted scores, stamped with the psyop name, and
/// with the `agents enqueue`-only columns absent.
fn assert_queue_rows(rows: &[QueueEntry], agent: &str, kind: AgentKind) {
    assert_eq!(rows.len(), 2, "{agent}: expected 2 queued tweets, got {rows:#?}");
    let expected = [("1900000000000000111", 0.7531_f64), ("1900000000000000222", 0.4218_f64)];
    for (tweet_id, score) in expected {
        let row = rows
            .iter()
            .find(|r| r.tweet_id == tweet_id)
            .unwrap_or_else(|| panic!("{agent}: missing tweet {tweet_id}: {rows:#?}"));
        assert_eq!(row.agent, agent);
        assert_eq!(row.agent_kind, kind);
        assert_eq!(row.psyop.as_deref(), Some("test-psyop"), "{agent}/{tweet_id}");
        assert_eq!(row.score, Some(score), "{agent}/{tweet_id}");
        assert_eq!(row.deliverer_agent_instance_hierarchy, None, "{agent}/{tweet_id}");
        assert_eq!(row.message, None, "{agent}/{tweet_id}");
    }
}
