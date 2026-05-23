//! Runtime that drives the stdio protocol. The wire types and event-
//! name constants live in `psychological-operations-browser-sdk`;
//! this module just plumbs them through:
//!
//! 1. A background thread reads one [`Request`] per line from stdin.
//! 2. Each parsed request is forwarded to the frontend via the
//!    [`sdk::stdio::EVENT_REQUEST`] Tauri event.
//! 3. The frontend handles the request and posts a [`Response`] back
//!    via the [`stdio_respond`] Tauri command.
//! 4. [`stdio_respond`] serializes the response as a JSON line and
//!    writes it to stdout under a mutex.

use std::io::{BufRead, Write};
use std::sync::{Mutex, OnceLock};

use psychological_operations_browser_sdk::stdio as sdk;
use psychological_operations_browser_sdk::stdio::{Request, Response};
use tauri::{AppHandle, Emitter, Runtime};

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
            if let Err(e) = handle.emit(sdk::EVENT_REQUEST, req) {
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
