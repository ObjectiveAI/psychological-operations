//! Set a write-quota limit and read it back — a partial assertion on the
//! `agents quota limit get` notification.

use psychological_operations_tests::{Agent, Dir, Plugin};

#[tokio::test]
async fn agents_quota_set_get() {
    let p = Plugin::new("agents_quota_set_get");

    p.agents_quota_limit_set(Agent::Me, Dir::Write, 2)
        .await
        .assert_ok();

    let got = p.agents_quota_limit_get(Agent::Me, Dir::Write).await;
    got.assert_no_errors();
    // `… get` emits a raw notification `{account, direction, limit}`.
    assert!(
        got.events.iter().any(|e| {
            e.get("limit").and_then(|v| v.as_u64()) == Some(2)
                && e.get("direction").and_then(|v| v.as_str()) == Some("write")
        }),
        "expected limit=2 write in events, got {:?}",
        got.events,
    );
}
