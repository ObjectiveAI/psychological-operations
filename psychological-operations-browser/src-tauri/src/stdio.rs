//! JSON-Lines stdio protocol for the browser.
//!
//! Flow:
//! 1. A background thread reads one [`Request`] per line from stdin.
//! 2. Each parsed request is forwarded to the frontend via the
//!    [`EVENT_REQUEST`] Tauri event.
//! 3. The frontend (eventually) handles the request and posts a
//!    [`Response`] back via the [`stdio_respond`] Tauri command.
//! 4. [`stdio_respond`] serializes the response as a JSON line and
//!    writes it to stdout under a mutex.
//!
//! Both wire types are externally tagged on a `"type"` field
//! (e.g. `{"type":"html"}` request → `{"type":"html","html":"…"}`
//! response). See [`request`] and [`response`] for the type definitions.

pub mod request;
pub mod response;

pub use request::Request;
pub use response::Response;

use std::io::{BufRead, Write};
use std::sync::{Mutex, OnceLock};

use tauri::{AppHandle, Emitter, Runtime};

/// Tauri event name the host emits requests to. The frontend
/// subscribes via `listen('stdio:request', ...)` (Tauri JS API).
pub const EVENT_REQUEST: &str = "stdio:request";

/// Spawns the stdin reader thread. Idempotent: each call spawns
/// another thread, so call exactly once from `setup`.
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

/// Tauri command the frontend invokes to send a response back to
/// the host process. Serializes the response as one JSON line on
/// stdout under a mutex (so concurrent responses don't interleave).
#[tauri::command]
pub fn stdio_respond(response: Response) -> Result<(), String> {
    let line = serde_json::to_string(&response).map_err(|e| e.to_string())?;
    let _guard = stdout_lock().lock().map_err(|e| e.to_string())?;
    let mut out = std::io::stdout().lock();
    writeln!(out, "{line}").map_err(|e| e.to_string())?;
    out.flush().map_err(|e| e.to_string())?;
    Ok(())
}
