//! Runtime that drives the stdio protocol. The wire types live in
//! `psychological-operations-browser-sdk`; this module handles the
//! Tauri-specific transport:
//!
//! 1. A background thread reads one [`Request`] per line from stdin.
//! 2. Each parsed request is forwarded to the frontend via the
//!    [`EVENT_REQUEST`] Tauri event.
//! 3. The frontend handles the request and posts a [`Response`] back
//!    via the [`stdio_respond`] Tauri command.
//! 4. [`stdio_respond`] serializes the response as a JSON line and
//!    writes it to stdout under a mutex.

use std::io::{BufRead, Write};
use std::sync::{Mutex, OnceLock};

use psychological_operations_browser_sdk::request::Request;
use psychological_operations_browser_sdk::response::Response;
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
                    eprintln!("stdio: dropping unparseable line: {e}");
                    continue;
                }
            };
            if let Err(e) = handle.emit(EVENT_REQUEST, req) {
                eprintln!("stdio: emit failed: {e}");
            }
        }
    });
}

fn stdout_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Tauri command the frontend invokes to send a response back to the
/// host process. Serializes the response as one JSON line on stdout
/// under a mutex (so concurrent responses don't interleave).
#[tauri::command]
pub fn stdio_respond(response: Response) -> Result<(), String> {
    let line = serde_json::to_string(&response).map_err(|e| e.to_string())?;
    let _guard = stdout_lock().lock().map_err(|e| e.to_string())?;
    let mut out = std::io::stdout().lock();
    writeln!(out, "{line}").map_err(|e| e.to_string())?;
    out.flush().map_err(|e| e.to_string())?;
    Ok(())
}
