//! Reply/quote delivery driver (browser side) — stage-2 framework.
//!
//! Driven by the `--deliver <json>` invocation. The items (a JSON array of
//! [`DeliverItem`]) are grouped by agent; each agent is a sequential
//! "session": open a CEF browser on that agent's `agent-<tag>` profile and
//! walk every reply/quote — navigate to the tweet, show the copy widget +
//! operator instructions via the overlay, await an auto-detected
//! completion (or skip on timeout / unrecognized state), and stream
//! [`Output::Delivered`]. Then tear the browser down and move to the next
//! agent.
//!
//! The completion-detection signals + the x.com selectors live in the
//! overlay's `deliver-helpers` and are the iteration points; this module
//! is the orchestration framework.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use psychological_operations_sdk::browser::deliver::DeliverItem;
use psychological_operations_sdk::browser::mode::Mode;
use psychological_operations_sdk::browser::output::Output;
use tauri::{AppHandle, Wry};
use tokio::sync::oneshot;

/// Settle delay after navigating before pushing the copy widget, giving
/// the page + overlay time to load. (Iteration point — replace with a real
/// overlay-ready signal once the detection logic matures.)
const NAV_SETTLE: Duration = Duration::from_secs(3);
const BROWSER_UP_TIMEOUT: Duration = Duration::from_secs(30);
const BROWSER_DOWN_TIMEOUT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(50);

/// Per-item completion-report registry, keyed by `(tweet_id, kind)`. The
/// driver registers a oneshot before pushing each item; the overlay's
/// `deliver_report` invoke fulfills it via [`report`].
fn pending() -> &'static Mutex<HashMap<(String, String), oneshot::Sender<bool>>> {
    static P: OnceLock<Mutex<HashMap<(String, String), oneshot::Sender<bool>>>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Overlay → Rust completion report for one item (`status` = `"done"` |
/// `"skip"`). Fulfills the driver's waiter for `(tweet_id, kind)`.
pub fn report(tweet_id: &str, kind: &str, status: &str) {
    let done = status == "done";
    if let Ok(mut map) = pending().lock() {
        if let Some(tx) = map.remove(&(tweet_id.to_string(), kind.to_string())) {
            let _ = tx.send(done);
        }
    }
}

/// Set once the driver has walked every agent and is about to exit, so the
/// `RunEvent::ExitRequested` guard in `lib.rs` stops holding the app open
/// and lets the final `handle.exit(0)` through.
static FINISHED: AtomicBool = AtomicBool::new(false);

/// True once the delivery batch is fully done (all agents walked).
pub fn is_finished() -> bool {
    FINISHED.load(Ordering::SeqCst)
}

/// Spawn the delivery driver task: run each agent session sequentially,
/// then exit the app so the CLI driver sees stdout EOF.
pub fn start(handle: AppHandle<Wry>, items: Vec<DeliverItem>) {
    tauri::async_runtime::spawn(async move {
        run(&handle, items).await;
        // Release the exit guard, then terminate so the CLI sees EOF.
        FINISHED.store(true, Ordering::SeqCst);
        handle.exit(0);
    });
}

async fn run(handle: &AppHandle<Wry>, items: Vec<DeliverItem>) {
    // Group by agent (BTreeMap → stable, sorted by tag). Replies/quotes are
    // walked in queue order within each agent.
    let mut by_agent: BTreeMap<String, Vec<DeliverItem>> = BTreeMap::new();
    for it in items {
        by_agent.entry(it.agent.clone()).or_default().push(it);
    }

    for (agent, group) in by_agent {
        // Open this agent's X session (its own CEF profile). The cache
        // subdir matches what `agents login` wrote (the canonical Mode
        // mapping).
        let cache_subdir = Mode::AgentBrowser { name: agent.clone() }.cache_subdir();
        crate::webview::spawn_agent_browser(handle, &cache_subdir, "https://x.com/home");
        if !wait_until(BROWSER_UP_TIMEOUT, crate::cef::has_browser).await {
            let _ = Output::Log {
                message: format!("deliver: browser for agent {agent} did not come up; skipping"),
            }
            .emit();
            continue;
        }

        for item in &group {
            deliver_one(item).await;
        }

        // Tear down this agent's browser before the next (the `do_close`
        // hook flushes its cookie store).
        if crate::cef::has_browser() {
            crate::cef::close_browser_async();
            let _ = wait_until(BROWSER_DOWN_TIMEOUT, || !crate::cef::has_browser()).await;
        }
    }
}

async fn deliver_one(item: &DeliverItem) {
    // Navigate to the target tweet (handle-less form). Iteration point.
    let url = format!("https://x.com/i/web/status/{}", item.tweet_id);
    crate::cef::navigate(url);
    tokio::time::sleep(NAV_SETTLE).await;

    // Register the completion waiter, then drive the overlay to render the
    // copy widget + start detecting.
    let key = (item.tweet_id.clone(), item.kind.clone());
    let (tx, rx) = oneshot::channel::<bool>();
    if let Ok(mut map) = pending().lock() {
        map.insert(key.clone(), tx);
    }
    push_item(item);

    // Wait indefinitely — delivery is operator-actuated; there is no
    // wall-clock timeout. The overlay resolves with `true` (posted) or
    // `false` (operator clicked "Skip"); an `Err` means the overlay went
    // away (navigation/teardown) without reporting.
    let outcome = rx.await;
    // Drop the waiter if still registered (e.g. the Err path).
    if let Ok(mut map) = pending().lock() {
        map.remove(&key);
    }
    match outcome {
        Ok(true) => {
            let _ = Output::Delivered {
                tweet_id: item.tweet_id.clone(),
                agent: item.agent.clone(),
                kind: item.kind.clone(),
            }
            .emit();
        }
        _ => {
            // skip / overlay gone — leave the queue row, move on.
            let _ = Output::Log {
                message: format!(
                    "deliver: skipped {} {} for {}",
                    item.kind, item.tweet_id, item.agent
                ),
            }
            .emit();
        }
    }
}

/// Drive the overlay's delivery entrypoint with one item — NOT the stdin
/// `Request` channel. The overlay's `deliver-helpers` registers
/// `window.__psyops_deliver`.
fn push_item(item: &DeliverItem) {
    let json = serde_json::to_string(item).unwrap_or_else(|_| "null".into());
    crate::cef::execute_overlay_js(format!(
        "window.__psyops_deliver && window.__psyops_deliver({json})"
    ));
}

/// Poll `cond` every [`POLL`] until it's true or `timeout` elapses.
async fn wait_until(timeout: Duration, cond: impl Fn() -> bool) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if cond() {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(POLL).await;
    }
}
