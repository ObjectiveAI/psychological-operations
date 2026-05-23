//! Tauri-side runtime for the JSON-Lines stdio protocol.
//!
//! 1. [`start`] spawns a background thread that reads lines from
//!    stdin and emits each parsed [`Request`] on [`EVENT_REQUEST`].
//! 2. The frontend handles the request and posts a [`ResponseOutcome`]
//!    back via the [`stdio_respond`] Tauri command.
//! 3. [`stdio_respond`] wraps the outcome in [`Output::Response`] and
//!    writes it via [`write_output`].
//! 4. [`write_output`] serializes the [`Output`] as one JSON line on
//!    stdout under a mutex (so concurrent writes don't interleave).
//!
//! The browser never writes to stdout or stderr outside `write_output`.

use std::io::{BufRead, Write};
use std::sync::{Mutex, OnceLock};

use psychological_operations_browser_sdk::output::{Output, ResponseOutcome};
use psychological_operations_browser_sdk::request::Request;
use tauri::{AppHandle, Emitter, Runtime};

/// Tauri event channel the browser emits stdio requests on.
/// Follows the `psyops:<topic>:<event>` naming convention.
pub const EVENT_REQUEST: &str = "psyops:stdio:request";

/// Spawns the stdin reader thread. Call exactly once from `setup`.
pub fn start<R: Runtime>(handle: AppHandle<R>) {
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let Ok(line) = line else { break };
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let req: Request = match serde_json::from_str(trimmed) {
                Ok(r) => r,
                Err(e) => {
                    let _ = write_output(&Output::Log {
                        message: format!("stdio: dropping unparseable line: {e}"),
                    });
                    continue;
                }
            };
            if let Err(e) = handle.emit(EVENT_REQUEST, req) {
                let _ = write_output(&Output::Log {
                    message: format!("stdio: emit failed: {e}"),
                });
            }
        }
    });
}

fn stdout_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Serialize an [`Output`] as one JSON line on stdout under a mutex.
/// The single channel through which every byte the browser writes
/// goes.
pub fn write_output(out: &Output) -> std::io::Result<()> {
    let line = serde_json::to_string(out)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let _guard = stdout_lock().lock().expect("stdout lock poisoned");
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{line}")?;
    stdout.flush()?;
    Ok(())
}

/// Tauri command the frontend invokes to deliver a response (ok or
/// err) back to the host. Wraps the outcome in [`Output::Response`]
/// and writes it via [`write_output`].
#[tauri::command]
pub fn stdio_respond(result: ResponseOutcome) -> Result<(), String> {
    write_output(&Output::Response { result }).map_err(|e| e.to_string())
}
