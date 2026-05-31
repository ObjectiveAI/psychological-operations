//! Verifies the auth two-tier locking semantics: cold-then-warm
//! `read_auth`, in-process thundering-herd serialization through
//! the tokio mutex tier, and release-ordering (SQLite row deleted
//! before the in-process guard drops).
//!
//! Doesn't exercise OAuth refresh end-to-end (that would require
//! a live token endpoint); the third subtest hand-rolls the
//! lock/write cycle to verify the lock pieces compose.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{Duration as ChronoDuration, Utc};

use psychological_operations_sdk::browser::auth_json::{PersonaKind, Tokens};
use psychological_operations_sdk::x::auth::PersonaKey;
use psychological_operations_sdk::x::client::{AuthMode, Client};

fn tmp_root(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "psyops-auth-{}-{}-{}",
        name,
        std::process::id(),
        unix_now_nanos(),
    ));
    p
}

fn unix_now_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// Seed an `x_app.json` under `config_base_dir` so `Client::new` with `AuthMode::XApp`
/// finds a bearer to use.
fn seed_x_app(config_base_dir: &std::path::Path) {
    use std::io::Write;
    let dir = config_base_dir
        .join("plugins")
        .join("psychological-operations");
    std::fs::create_dir_all(&dir).unwrap();
    let mut f = std::fs::File::create(dir.join("x_app.json")).unwrap();
    f.write_all(
        br#"{
            "bearer_token": "test-bearer",
            "client_id": "test-cid",
            "client_secret": "test-secret"
        }"#,
    )
    .unwrap();
}

fn fake_tokens(label: &str) -> Tokens {
    Tokens {
        access_token: format!("access-{label}"),
        refresh_token: Some(format!("refresh-{label}")),
        expires_at: Utc::now() + ChronoDuration::hours(1),
        scope: "tweet.read users.read".into(),
        saved_at: Utc::now(),
    }
}

fn fake_persona() -> PersonaKey {
    PersonaKey {
        kind: PersonaKind::Psyop,
        name: "alice".into(),
        persona_twid: "100".into(),
        x_app_twid: "200".into(),
    }
}

async fn build_client(root: &std::path::Path) -> Client {
    seed_x_app(root);
    Client::new(
        reqwest::Client::new(),
        false,
        256 * 1024 * 1024,
        Duration::from_secs(3600),
        root.to_path_buf(),
        AuthMode::XApp,
    )
}

#[tokio::test]
async fn cold_then_warm_read() {
    let root = tmp_root("read");
    let client = build_client(&root).await;
    let persona = fake_persona();

    // Cold: no auth.json on disk → None.
    let r = client.read_auth(&persona).expect("read ok");
    assert!(r.is_none(), "expected None on cold cache, got {r:?}");

    // Lock + write a token bundle.
    let tokens = fake_tokens("first");
    let lock = client.lock_auth(&persona).await.expect("lock");
    client.write_auth(lock, &tokens).await.expect("write");

    // Warm: read returns what we wrote.
    let r = client.read_auth(&persona).expect("read ok");
    let got = r.expect("auth.json present after write");
    assert_eq!(got.access_token, "access-first");
    assert_eq!(got.refresh_token.as_deref(), Some("refresh-first"));
}

#[tokio::test]
async fn in_process_thundering_herd() {
    // 30 in-process tasks all want to acquire the auth lock for the
    // same persona. Without the in-process tier, every contender
    // would do the SQLite acquire dance; the DashMap tier means
    // they serialize through tokio::sync::Mutex with sub-millisecond
    // hand-offs.
    //
    // Each task acquires + holds 2 ms + releases. With the
    // in-process tier in front, contenders queue on the local tokio
    // Mutex; only the winner runs the SQLite acquire transaction,
    // and the next contender sees a clean `locks` table after the
    // winner's awaited DELETE. Without the in-process tier, all
    // 30 would race for the SQLite row, lose, and spin-poll the
    // 50 ms tick — ~25 × 50 ms average wait per task → multi-second.
    // The bound below is a soft ceiling: well above realistic
    // Windows SQLite-per-acquire latency (~10–20 ms × 30 = 300–
    // 600 ms) but well below the poll-spin pathology (>1.5 s).
    let root = tmp_root("herd");
    let client = Arc::new(build_client(&root).await);
    let persona = Arc::new(fake_persona());

    let start = Instant::now();
    let mut handles = Vec::new();
    for _ in 0..30 {
        let client = client.clone();
        let persona = persona.clone();
        handles.push(tokio::spawn(async move {
            let lock = client.lock_auth(&persona).await.expect("lock");
            // Hold briefly to force the next contender to wait.
            tokio::time::sleep(Duration::from_millis(2)).await;
            drop(lock);
        }));
    }
    for h in handles {
        h.await.expect("task");
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_millis(1500),
        "30-task auth lock herd took {elapsed:?}; expected sub-1.5s \
         (poll-spin pathology would be multi-second)",
    );
}

#[tokio::test]
async fn release_ordering_no_spin_poll() {
    // Task A acquires + writes (which releases). Task B should
    // acquire its lock within the in-process mutex hand-off
    // latency — NOT have to wait for the 50 ms SQLite poll tick.
    let root = tmp_root("order");
    let client = Arc::new(build_client(&root).await);
    let persona = Arc::new(fake_persona());

    let (tx_a_done, rx_a_done) = tokio::sync::oneshot::channel::<Instant>();

    let client_a = client.clone();
    let persona_a = persona.clone();
    let a = tokio::spawn(async move {
        let lock = client_a.lock_auth(&persona_a).await.expect("A lock");
        tokio::time::sleep(Duration::from_millis(100)).await;
        // write_auth releases the lock as part of completion (DELETE
        // then drop inproc, in that order).
        client_a
            .write_auth(lock, &fake_tokens("a"))
            .await
            .expect("A write");
        let _ = tx_a_done.send(Instant::now());
    });

    // Give A a head start so it holds the lock before B asks.
    tokio::time::sleep(Duration::from_millis(20)).await;

    let client_b = client.clone();
    let persona_b = persona.clone();
    let b = tokio::spawn(async move {
        let _g = client_b.lock_auth(&persona_b).await.expect("B lock");
        Instant::now()
    });

    let a_done_at = rx_a_done.await.expect("A signaled");
    let b_acquired_at = b.await.expect("B task");
    a.await.expect("A task");

    let handoff = b_acquired_at.saturating_duration_since(a_done_at);
    assert!(
        handoff < Duration::from_millis(60),
        "B acquired {handoff:?} after A's write_auth returned — \
         should be sub-poll-tick (SQLite row was awaited-deleted \
         before inproc dropped)",
    );
}
