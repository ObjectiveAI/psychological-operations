//! Shared stream-and-shutdown plumbing for browser spawn-and-wait
//! flows. Both `login` (psyops/agents) and `x-app setup` do the
//! same dance:
//!
//!   1. Spawn the browser with stdin + stdout piped.
//!   2. Read the child's stdout line-by-line, parsing each into
//!      [`psychological_operations_sdk::browser::output::Output`].
//!      Stop on the first matching terminator; forward
//!      [`Output::Error`] to stderr; silently drop everything
//!      else (no spamming the operator's terminal with browser-
//!      side `Log`/`Panel`/`Url`/`SignedIn`/`TweetId` chatter).
//!   3. Send [`psychological_operations_sdk::browser::request::Request::Shutdown`]
//!      over the child's stdin (best-effort).
//!
//! The shape is parameterized on the terminator predicate so the
//! caller decides which `Output` variants count as success or
//! failure.

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout};

use psychological_operations_sdk::browser::output::Output;
use psychological_operations_sdk::browser::request::Request;

/// Stream the child's piped stdout. On each parsed [`Output`],
/// call `is_terminator`. If it returns `Some(result)`, stop
/// reading and return that result. [`Output::Error`] lines are
/// forwarded to the caller's stderr; every other variant is
/// silently dropped.
///
/// EOF before any terminator → `Err(eof_message.to_string())`.
/// JSON-parse failures on a line → silently dropped (the browser
/// might emit lines under future schema versions; better to
/// keep reading than to crash).
pub async fn watch_for_terminator<F, T>(
    stdout: ChildStdout,
    eof_message: &str,
    is_terminator: F,
) -> Result<T, String>
where
    F: Fn(&Output) -> Option<Result<T, String>>,
{
    let mut lines = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(output) = serde_json::from_str::<Output>(&line) else {
            continue;
        };
        if let Some(result) = is_terminator(&output) {
            return result;
        }
        if let Output::Error { error } = output {
            crate::output::OutputResult::from(crate::events::Event::BrowserError { error }).emit();
        }
    }
    Err(eof_message.to_string())
}

/// Send [`Request::Shutdown`] over the child's piped stdin
/// (best-effort) and close stdin so the browser's stdio reader
/// sees EOF if needed.
///
/// If the child has already exited, the write fails silently —
/// the caller's subsequent `child.wait()` reaps it.
pub async fn send_shutdown(mut stdin: ChildStdin) {
    let req = serde_json::to_string(&Request::Shutdown)
        .expect("Request::Shutdown is always serializable");
    let _ = stdin.write_all(format!("{req}\n").as_bytes()).await;
    let _ = stdin.flush().await;
    drop(stdin);
}
