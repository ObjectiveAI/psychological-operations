//! JSON-Lines stdin/stdout dev API.
//!
//! Reads one JSON object per line from stdin, parses, dispatches to
//! UI-thread handlers, and writes one JSON response per line to stdout
//! (under a mutex so concurrent in-flight responses don't interleave).
//!
//! Supported request types:
//!   {"type":"url"}  → {"type":"url","url":"…"}   or {"type":"url","error":"…"}
//!   {"type":"html"} → {"type":"html","html":"…"} or {"type":"html","error":"…"}

use std::io::{BufRead, Write};
use std::sync::{Mutex, OnceLock};

use serde::Deserialize;
use serde_json::json;
use tauri::{AppHandle, Manager, Runtime, WebviewWindow};

const WINDOW_LABEL: &str = "main";

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Request {
    Url,
    Html,
}

fn stdout_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn write_line(value: serde_json::Value) {
    let line = value.to_string();
    let lock = stdout_lock().lock().expect("stdout lock poisoned");
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
    drop(lock);
}

pub fn start<R: Runtime>(handle: AppHandle<R>) {
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let Ok(line) = line else { break; };
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Ok(req) = serde_json::from_str::<Request>(trimmed) else {
                eprintln!("psyops stdin: dropping unparseable line");
                continue;
            };
            let h = handle.clone();
            let _ = handle.run_on_main_thread(move || dispatch(&h, req));
        }
    });
}

fn dispatch<R: Runtime>(handle: &AppHandle<R>, req: Request) {
    let win: Option<WebviewWindow<R>> = handle.get_webview_window(WINDOW_LABEL);
    let Some(win) = win else {
        write_line(json!({ "type": type_str(&req), "error": "no main window" }));
        return;
    };

    match req {
        Request::Url => {
            match win.url() {
                Ok(u) => write_line(json!({ "type": "url", "url": u.to_string() })),
                Err(e) => write_line(json!({ "type": "url", "error": e.to_string() })),
            }
        }
        Request::Html => {
            // Eval JS that posts the outerHTML back via our Tauri command.
            let script = "window.__TAURI__.core.invoke('psyops_html_response', \
                          { html: document.documentElement.outerHTML });";
            if let Err(e) = win.eval(script) {
                write_line(json!({ "type": "html", "error": e.to_string() }));
            }
        }
    }
}

fn type_str(req: &Request) -> &'static str {
    match req {
        Request::Url => "url",
        Request::Html => "html",
    }
}

/// Tauri command invoked by the JS half of the {type:"html"} request.
/// Writes the JSON response line to stdout.
#[tauri::command]
pub fn psyops_html_response(html: String) {
    write_line(json!({ "type": "html", "html": html }));
}
