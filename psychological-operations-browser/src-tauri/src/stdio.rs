//! Tauri-side runtime for the JSON-Lines stdio protocol.
//!
//! 1. [`start`] spawns a background thread that reads lines from
//!    stdin, parses each one as a [`Request`], and dispatches it to
//!    [`dispatch_request`].
//! 2. Each request handler is responsible for emitting exactly one
//!    [`Output::Response`] (ok or err). Handlers that need to do
//!    main-thread work (e.g. creating a webview) hop via
//!    [`AppHandle::run_on_main_thread`] and a sync channel.
//! 3. The injected overlay reports SPA URL changes via the
//!    [`report_url`] Tauri command, which emits [`Output::Url`].
//!
//! Every byte the browser writes goes through [`Output::emit`] —
//! no `println!`, no `eprintln!`, no direct stderr from our code.

use std::io::BufRead;
use std::sync::mpsc;

use psychological_operations_browser_sdk::output::Output;
use psychological_operations_browser_sdk::request::Request;
use psychological_operations_browser_sdk::response::{Response, ResponseOutcome};
use tauri::{AppHandle, Runtime};

use crate::webview;

/// Spawn the stdin reader thread. Call exactly once from `setup`.
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
            dispatch_request(&handle, req);
        }
    });
}

fn dispatch_request<R: Runtime>(handle: &AppHandle<R>, req: Request) {
    let outcome = match req {
        Request::XApp => handle_x_app(handle),
        Request::Html => {
            let _ = Output::Log {
                message: "stdio: html request not implemented yet".into(),
            }
            .emit();
            ResponseOutcome::Err {
                error: "html request not implemented".into(),
            }
        }
    };
    let _ = Output::Response { result: outcome }.emit();
}

fn handle_x_app<R: Runtime>(handle: &AppHandle<R>) -> ResponseOutcome {
    let (tx, rx) = mpsc::channel();
    let h = handle.clone();
    let dispatched = handle.run_on_main_thread(move || {
        let res = webview::create_x_app(&h)
            .map(|_| ())
            .map_err(|e| e.to_string());
        let _ = tx.send(res);
    });
    match dispatched {
        Ok(()) => match rx.recv() {
            Ok(Ok(())) => ResponseOutcome::Ok {
                response: Response::Ack,
            },
            Ok(Err(error)) => ResponseOutcome::Err { error },
            Err(e) => ResponseOutcome::Err {
                error: format!("webview create channel: {e}"),
            },
        },
        Err(e) => ResponseOutcome::Err {
            error: format!("dispatch failed: {e}"),
        },
    }
}

/// Tauri command the injected overlay invokes on every URL change
/// inside the X webview (SPA route changes via `history.pushState` /
/// `popstate`). Native full-page navigations route through the
/// `on_navigation` callback in [`crate::webview`] instead.
#[tauri::command]
pub fn report_url(url: String) -> Result<(), String> {
    Output::Url { url }.emit().map_err(|e| e.to_string())
}
