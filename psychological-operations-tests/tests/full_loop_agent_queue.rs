//! Full end-to-end loop across BOTH command surfaces — the psychological-
//! operations PLUGIN and the objectiveai HOST root:
//!
//! 1. (HOST)   register a tag bound to an inline mock agent
//! 2. (PLUGIN) publish a queries-only psyop (no scoring)
//! 3. (PLUGIN) add a global `agent_queue` target → the tag
//! 4. (PLUGIN) full global `psyops run` — survivors land in the agent queue
//! 5. (PLUGIN) `agents notify` — enqueues ONE objectiveai notification per
//!             agent (no spawn)
//! 6. (HOST)   `agents queue deliver --stream` — spawns + runs the mock
//!             agent synchronously, delivering the notification
//! 7. (HOST)   `agents instances list` → exactly one instance
//! 8. (HOST)   `agents logs read pending` → the delivered ClientNotification
//! 9. (HOST)   `agents logs read id <id>` → assert the text is exactly the
//!             notification `agents notify` enqueued

use objectiveai_sdk::cli::command::agents::logs::read::all::{
    ClientNotificationPartType, ResponseItem as LogItem,
};
use objectiveai_sdk::cli::command::agents::logs::read::id::Response as LogById;
use objectiveai_sdk::cli::command::agents::queue::deliver::ResponseItem as DeliverItem;
use psychological_operations_sdk::cli::destinations::Destination;
use psychological_operations_sdk::cli::destinations::agent_queue::AgentQueue;
use psychological_operations_tests::{Plugin, Selector, Target, query_psyop};

#[tokio::test]
async fn full_loop_agent_queue() {
    let tag = "loop-agent";
    let p = Plugin::new("full_loop_agent_queue");

    // 1. Register the tag → a fresh inline mock agent (deterministic, no
    //    upstream LLM). Creates a GROUPED tag carrying the mock spec.
    p.agents_tags_apply_mock(tag).await.assert_no_errors();

    // 2. A queries-only psyop (no scoring stages → every ingested post is a
    //    survivor at max score).
    p.psyops_publish("test-psyop", &query_psyop("mock fallback search", vec![]))
        .await
        .assert_no_errors();

    // 3. Global `agent_queue` target pointing at the tag — without it the
    //    psyop has nothing to deliver to the agent's queue.
    p.targets_add(
        Selector::Global,
        &Destination::AgentQueue(AgentQueue::AgentTag { agent_tag: tag.to_string() }),
    )
    .await
    .assert_ok();

    // 4. Full global run. Ingests via the mock query and (draining its
    //    targets) enqueues one psyops-queue row per survivor against the
    //    tag. No scoring ⇒ survivors == ingested == Σ query_complete counts.
    let run = p.psyops_run(&[], Some(42)).await;
    run.assert_no_errors();
    let n: i64 = run
        .events
        .iter()
        .filter(|e| e.get("event").and_then(|v| v.as_str()) == Some("query_complete"))
        .filter_map(|e| e.get("count").and_then(|v| v.as_i64()))
        .sum();
    assert!(n >= 2, "expected the mock query to ingest >= 2 posts, got {n}");

    // 5. Notify — enqueues ONE objectiveai notification for the tag (no
    //    spawn). This is the exact string we assert on at the end.
    let expected = format!("The account \"{tag}\" has {n} tweets in the queue.");
    p.agents_notify().await.assert_ok();

    // 6. Deliver the queue in stream mode — the synchronous spawn: wins the
    //    tag's lock, runs the mock agent in-process, delivers the
    //    notification, and reaches `AllAgentsActive` only once done.
    let deliver = p.agents_queue_deliver_stream().await;
    deliver.assert_no_errors();
    assert!(
        deliver.items.iter().any(|i| matches!(i, DeliverItem::AllAgentsActive(_))),
        "queue deliver --stream should reach AllAgentsActive",
    );

    // 7. Exactly one agent instance under the root.
    let instances = p.agents_instances_list(vec![Target::Me]).await;
    instances.assert_no_errors();
    assert_eq!(
        instances.items.len(),
        1,
        "expected exactly one agent instance, got {}",
        instances.items.len(),
    );

    // 8. The spawned agent's logs hold the delivered ClientNotification —
    //    read them by tag (the agent's logs live under its own instance,
    //    not the caller's `me`) and take its text part's id
    //    (`logs.messages."index"`).
    let logs = p.agents_logs_read_all(vec![Target::Tag { agent_tag: tag.to_string() }]).await;
    logs.assert_no_errors();
    let id = logs
        .items
        .iter()
        .find_map(|item| match item {
            LogItem::ClientNotification { parts, .. } => parts
                .iter()
                .find(|part| matches!(part.r#type, ClientNotificationPartType::Text))
                .map(|part| part.id),
            _ => None,
        })
        .expect("a ClientNotification text part among the agent's logs");

    // 9. Read that row by id and assert the text is exactly the notification
    //    `agents notify` enqueued.
    let by_id = p.agents_logs_read_id(id).await;
    by_id.assert_no_errors();
    let text = by_id
        .items
        .iter()
        .find_map(|r| match r {
            LogById::Text { text } => Some(text.clone()),
            _ => None,
        })
        .expect("a Text response from `agents logs read id`");
    assert_eq!(text, expected);
}
