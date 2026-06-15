//! End-to-end: an agent actually HANDLES the queue through the MCP server.
//!
//! A scripted mock agent (deterministic tool call) is bound to a tag, a
//! tweet is parked on that tag's psyops queue, and the agent — spawned via
//! plain `agents spawn` (streaming, NOT queue deliver) — calls the
//! psychological-operations x-api MCP tool `mark_handled` with the enqueued
//! tweet id. This exercises the MCP server end-to-end: tool exposure (FULL
//! mode), the tool call, the server's response, and the queue mutation.
//!
//! Steps:
//! 1. (HOST)   apply a tag → scripted mock agent (calls mark_handled)
//! 2. (PLUGIN) `agents enqueue` one tweet onto the tag's psyops queue
//! 3. (PLUGIN) `agents notify` → enqueues ONE objectiveai notification
//! 4. (HOST)   `agents spawn` (empty message, streaming) → agent calls
//!             mark_handled; the MCP server removes the tweet
//!     → assert the tool call + the MCP server's response in the stream
//!     → assert the delivered notification text (same as the agent-queue test)
//! 5. (PLUGIN) `agents notify` again — psyops queue now empty, so no-op
//! 6. (HOST)   `agents queue read pending` is empty (no enqueuement occurred)

use objectiveai_sdk::cli::command::agents::logs::read::all::{
    ClientNotificationPartType, ResponseItem as LogItem,
};
use objectiveai_sdk::cli::command::agents::logs::read::id::Response as LogById;
use psychological_operations_tests::{Agent, Plugin, Target, mark_handled_mock_agent};

#[tokio::test]
async fn full_loop_agent_handles_queue() {
    let tag = "handler-agent";
    let tweet_id = "1730000000000000001";
    let p = Plugin::new("full_loop_agent_handles_queue");

    // 1. Bind the tag to a scripted mock agent that calls
    //    `mark_handled(account=tag, tweet_ids=[tweet_id])` in FULL mode.
    p.agents_tags_apply_inline(tag, mark_handled_mock_agent(tag, &[tweet_id]))
        .await
        .assert_no_errors();

    // 2. Park one tweet on the tag's psyops queue.
    p.agents_enqueue(Agent::Tag(tag), tweet_id, "handle this please")
        .await
        .assert_ok();

    // 3. Notify → enqueues ONE objectiveai notification ("…has 1 tweets…").
    p.agents_notify().await.assert_ok();

    // 4. Plain spawn (empty message, streaming) — drains the notification,
    //    the agent calls mark_handled, the MCP server removes the tweet.
    let spawn = p.agents_spawn_stream(tag).await;
    spawn.assert_no_errors();

    // 4a. Assert the correct tool call + the MCP server's response. Streaming
    //     chunks fragment fields across items, so match the serialized stream.
    let stream = spawn
        .items
        .iter()
        .map(|i| serde_json::to_value(i).expect("spawn item serializes").to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        stream.contains("psychological-operations-x-api_mark_handled"),
        "expected the mark_handled tool call in the spawn stream",
    );
    assert!(
        stream.contains(tweet_id),
        "expected the enqueued tweet id as the tool argument in the spawn stream",
    );
    assert!(
        stream.contains("removed"),
        "expected the MCP server's {{\"removed\":N}} response in the spawn stream",
    );

    // 4b. Same notification assertion as the agent-queue test: the delivered
    //     ClientNotification (in the agent's own logs, read by tag) reads
    //     back as the exact `agents notify` text.
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
    assert_eq!(text, format!("The account \"{tag}\" has 1 tweets in the queue."));

    // 5. Notify again — the agent emptied the psyops queue, so this finds
    //    nothing to enqueue.
    p.agents_notify().await.assert_ok();

    // 6. The objectiveai message_queue is empty: the first notification was
    //    consumed by the spawn, and notify #2 enqueued nothing because the
    //    agent's mark_handled call already drained the psyops queue.
    let queue = p.agents_queue_read_pending(vec![Target::Me]).await;
    queue.assert_no_errors();
    assert!(
        queue.items.is_empty(),
        "objectiveai message_queue should be empty (agent handled the queue), got {:?}",
        queue.items,
    );
}
