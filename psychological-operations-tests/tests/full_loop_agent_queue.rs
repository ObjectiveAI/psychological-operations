//! Full end-to-end loop across BOTH command surfaces — the psychological-
//! operations PLUGIN and the objectiveai HOST root:
//!
//! 1. (HOST)   register a tag bound to an inline mock agent
//! 2. (PLUGIN) publish a queries-only psyop (no scoring) whose `agent_tags`
//!             names the tag — survivors deliver to its queue on run
//! 3. (PLUGIN) full global `psyops run` — survivors land in the agent's
//!             queue and the agent is notified of its new pending count
//! 4. (HOST)   `agents queue deliver --stream` — spawns + runs the mock
//!             agent synchronously, delivering the notification
//! 5. (HOST)   `agents instances list` → exactly one instance
//! 6. (HOST)   `agents logs read pending` → the delivered ClientNotification
//! 7. (HOST)   `agents logs read id <id>` → assert the text is exactly the
//!             notification the run enqueued

use objectiveai_sdk::cli::command::agents::logs::read::all::{
    ClientNotificationPartType, ResponseItem as LogItem,
};
use objectiveai_sdk::cli::command::agents::logs::read::id::Response as LogById;
use objectiveai_sdk::cli::command::agents::queue::deliver::ResponseItem as DeliverItem;
use psychological_operations_tests::{Plugin, Target, query_psyop};

#[tokio::test]
async fn full_loop_agent_queue() {
    let tag = "loop-agent";
    let p = Plugin::new("full_loop_agent_queue");

    // 1. Register the tag → a fresh inline mock agent (deterministic, no
    //    upstream LLM). Creates a GROUPED tag carrying the mock spec.
    p.agents_tags_apply_mock(tag).await.assert_no_errors();

    // 2. A queries-only psyop (no scoring stages → every ingested post is a
    //    survivor at max score) whose `agent_tags` names the tag — so the run
    //    delivers survivors to that agent's queue and notifies it.
    let mut psyop = query_psyop("mock fallback search", vec![]);
    psyop.agent_tags = vec![tag.to_string()];
    p.psyops_publish("test-psyop", &psyop)
        .await
        .assert_no_errors();

    // 3. Full global run. Ingests via the mock query and, because the psyop
    //    lists `agent_tags`, enqueues one psyops-queue row per survivor
    //    against the tag, then notifies the agent of its new pending count.
    //    No scoring ⇒ survivors == ingested == Σ query_complete counts.
    let run = p.psyops_run(&[], Some(42)).await;
    run.assert_no_errors();
    let n: i64 = run
        .events
        .iter()
        .filter(|e| e.get("event").and_then(|v| v.as_str()) == Some("query_complete"))
        .filter_map(|e| e.get("count").and_then(|v| v.as_i64()))
        .sum();
    assert!(
        n >= 2,
        "expected the mock query to ingest >= 2 posts, got {n}"
    );

    // The notification the run enqueued — the exact string we assert at the end.
    let expected = format!("The agent \"{tag}\" has {n} tweets in the queue.");

    // 4. Deliver the queue in stream mode — the synchronous spawn: wins the
    //    tag's lock, runs the mock agent in-process, delivers the
    //    notification, and reaches `AllAgentsActive` only once done.
    let deliver = p.agents_queue_deliver_stream().await;
    deliver.assert_no_errors();
    assert!(
        deliver
            .items
            .iter()
            .any(|i| matches!(i, DeliverItem::AllAgentsActive(_))),
        "queue deliver --stream should reach AllAgentsActive",
    );

    // 5. Exactly one agent instance under the root.
    let instances = p.agents_instances_list(vec![Target::Me]).await;
    instances.assert_no_errors();
    assert_eq!(
        instances.items.len(),
        1,
        "expected exactly one agent instance, got {}",
        instances.items.len(),
    );

    // 6. The spawned agent's logs hold the delivered ClientNotification —
    //    read them by tag (the agent's logs live under its own instance,
    //    not the caller's `me`) and take its text part's id
    //    (`logs.messages."index"`).
    let logs = p
        .agents_logs_read_all(vec![Target::Tag {
            agent_tag: tag.to_string(),
        }])
        .await;
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

    // 7. Read that row by id and assert the text is exactly the notification
    //    the run enqueued.
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
