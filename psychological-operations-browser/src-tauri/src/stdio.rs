//! Tauri-side runtime for the JSON-Lines stdio protocol.
//!
//! 1. [`start`] spawns a background thread that reads lines from
//!    stdin and emits each parsed [`Request`] on [`EVENT_REQUEST`].
//! 2. The frontend handles the request and posts a [`ResponseOutcome`]
//!    back via the [`stdio_respond`] Tauri command.
//! 3. [`stdio_respond`] wraps the outcome in [`Output::Response`]
//!    and writes it via [`Output::emit`].
//!
//! Every byte the browser writes goes through [`Output::emit`] —
//! no `println!`, no `eprintln!`, no direct stderr from our code.

use std::io::BufRead;

use psychological_operations_browser_sdk::output::Output;
use psychological_operations_browser_sdk::request::Request;
use psychological_operations_browser_sdk::response::ResponseOutcome;
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
                    let _ = Output::Log {
                        message: format!("stdio: dropping unparseable line: {e}"),
                    }
                    .emit();
                    continue;
                }
            };
            if let Err(e) = handle.emit(EVENT_REQUEST, req) {
                let _ = Output::Log {
                    message: format!("stdio: emit failed: {e}"),
                }
                .emit();
            }
        }
    });
}

/// Tauri command the frontend invokes to deliver a response (ok or
/// err) back to the host. Wraps the outcome in [`Output::Response`]
/// and writes it via [`Output::emit`].
#[tauri::command]
pub fn stdio_respond(result: ResponseOutcome) -> Result<(), String> {
    Output::Response { result }
        .emit()
        .map_err(|e| e.to_string())
}
