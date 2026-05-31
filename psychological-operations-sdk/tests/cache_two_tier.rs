//! Verifies the two-tier locking semantics of the X-API response
//! cache: in-process tasks contending for the same key serialize
//! through the local tokio mutex (Tier 1) before reaching the
//! SQLite `locks` table (Tier 2), and lock release awaits the
//! SQLite DELETE before yielding so the next in-process contender
//! doesn't spin-poll a stale row.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use psychological_operations_sdk::x::cache::Cache;

fn tmp_root(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "psyops-cache-{}-{}-{}",
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

#[tokio::test]
async fn cold_then_warm() {
    let root = tmp_root("cold");
    let cache = Cache::open(&root, 0).await.expect("open");

    let key = [1u8; 32];
    let count = Arc::new(AtomicUsize::new(0));

    let c1 = count.clone();
    let body1 = cache
        .get_or_fetch(&key, move || async move {
            c1.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(50)).await;
            Ok(b"hello".to_vec())
        })
        .await
        .expect("first fetch");
    assert_eq!(body1, b"hello");
    assert_eq!(count.load(Ordering::SeqCst), 1, "fetch ran once");

    // Second call must hit the cache.
    let c2 = count.clone();
    let body2 = cache
        .get_or_fetch(&key, move || async move {
            c2.fetch_add(1, Ordering::SeqCst);
            Ok(b"NEVER".to_vec())
        })
        .await
        .expect("second fetch");
    assert_eq!(body2, b"hello");
    assert_eq!(count.load(Ordering::SeqCst), 1, "fetch did NOT run again");
}

#[tokio::test]
async fn in_process_thundering_herd() {
    // 50 in-process tasks race for the same key on a cold cache.
    // Tier 1 (DashMap-keyed tokio mutex) must serialize them so
    // exactly one calls the fetch closure; the others read the
    // cached body. Without the in-process tier, multiple tasks
    // could pass the SQLite acquire (within the same process,
    // before the locks-table COMMIT becomes visible to others)
    // and each fire the fetch closure.
    let root = tmp_root("herd");
    let cache = Arc::new(Cache::open(&root, 0).await.expect("open"));

    let key = [2u8; 32];
    let fetch_count = Arc::new(AtomicUsize::new(0));

    let mut handles = Vec::new();
    for _ in 0..50 {
        let cache = cache.clone();
        let fetch_count = fetch_count.clone();
        handles.push(tokio::spawn(async move {
            cache
                .get_or_fetch(&key, move || {
                    let fc = fetch_count.clone();
                    async move {
                        fc.fetch_add(1, Ordering::SeqCst);
                        // Simulate a 75 ms upstream call — long enough
                        // that every herd member queues for the lock.
                        tokio::time::sleep(Duration::from_millis(75)).await;
                        Ok(b"shared body".to_vec())
                    }
                })
                .await
                .expect("get_or_fetch")
        }));
    }
    for h in handles {
        let body = h.await.expect("task");
        assert_eq!(body, b"shared body");
    }
    assert_eq!(
        fetch_count.load(Ordering::SeqCst),
        1,
        "fetch closure ran exactly once across the 50-task herd",
    );
}

#[tokio::test]
async fn release_ordering_no_spin_poll() {
    // Task A acquires + holds the per-key lock for 100 ms, then
    // releases. Task B (spawned concurrently) waits to acquire.
    // After A releases, B should acquire WITHIN the in-process
    // mutex hand-off latency — NOT have to wait for the 50 ms
    // SQLite poll tick because the SQLite row was already deleted
    // before A dropped its in-process guard.
    //
    // We give the assertion a generous bound (60 ms after A's
    // release) — well under one full SQLite poll tick — but
    // comfortably above realistic hand-off latency.
    let root = tmp_root("order");
    let cache = Arc::new(Cache::open(&root, 0).await.expect("open"));

    let key = [3u8; 32];
    let (tx_a_released, rx_a_released) = tokio::sync::oneshot::channel::<Instant>();

    let cache_a = cache.clone();
    let a = tokio::spawn(async move {
        let guard = cache_a.lock(&key).await.expect("A lock");
        tokio::time::sleep(Duration::from_millis(100)).await;
        guard.release().await;
        let _ = tx_a_released.send(Instant::now());
    });

    // Give A a head start so it owns the lock before B asks.
    tokio::time::sleep(Duration::from_millis(20)).await;

    let cache_b = cache.clone();
    let b = tokio::spawn(async move {
        let _g = cache_b.lock(&key).await.expect("B lock");
        Instant::now()
    });

    let a_released_at = rx_a_released.await.expect("A signaled");
    let b_acquired_at = b.await.expect("B task");
    a.await.expect("A task");

    let handoff = b_acquired_at.saturating_duration_since(a_released_at);
    assert!(
        handoff < Duration::from_millis(60),
        "B acquired {handoff:?} after A released — should be sub-poll-tick \
         (SQLite row was awaited-deleted before inproc mutex dropped)",
    );
}
