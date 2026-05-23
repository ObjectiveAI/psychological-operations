//! Everything the browser writes to stdout.
//!
//! Wire format: one JSON object per line, externally tagged on
//! `"type"`, with a top-level `"mode"` field carrying the current
//! [`crate::mode::Mode`] (or `null` if no mode has been set yet —
//! e.g. `--help` / clap-error lines emitted before the Tauri builder
//! starts). The browser never prints to stdout or stderr outside
//! [`Output::emit`] — all output flows through here.

use std::io::Write;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use crate::response::ResponseOutcome;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Output {
    /// A reply to a previously-received [`crate::request::Request`].
    /// The inner [`ResponseOutcome`] is either an `ok` response or an
    /// `err` with a textual reason.
    Response {
        #[serde(flatten)]
        result: ResponseOutcome,
    },

    /// `--help` / `--version` text. Emitted with exit code 0; the
    /// browser doesn't continue past this.
    Help { text: String },

    /// A fatal startup error (e.g. clap parse failure, mode
    /// validation). Emitted with a non-zero exit code; the browser
    /// doesn't continue past this.
    Error { error: String },

    /// A non-fatal diagnostic — anything the browser would have
    /// otherwise written to stderr at runtime (parse errors,
    /// lifecycle traces, etc.).
    Log { message: String },

    /// The URL the active content webview is currently on. Emitted
    /// on the initial overlay mount + every SPA route change
    /// (`history.pushState` / `replaceState` / `popstate` /
    /// `hashchange`) — see `src/spa-url.ts`.
    Url { url: String },

    /// Sign-in state of the current session. Emitted once on
    /// startup (after the watcher's initial cookie read), then
    /// again every time the auth cookie's presence-or-value
    /// changes. `info` carries identifying claims decoded from the
    /// auth JWT when signed in; absent when signed out.
    SignedIn {
        signed_in: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        info: Option<SignedInInfo>,
    },
}

/// Identifying claims extracted from the auth JWT's payload. All
/// fields are best-effort — the JWT may not carry any of them, in
/// which case the value is `None`. Extensible: adding new claim
/// fields here doesn't break the wire.
///
/// As of writing, xAI's `sso` JWT carries only `session_id` — no
/// human-friendly handle, no email, no stable user id. The other
/// fields are kept for forward-compat (and for future modes where
/// the auth token may carry richer identity).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedInInfo {
    /// Session identifier (UUID). For the X-App `sso` JWT this is
    /// the `session_id` claim — the only claim the token actually
    /// carries today.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// User handle / username — the human-friendly identifier.
    /// `None` if the JWT doesn't carry one (current state for xAI).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
    /// Email address if present in the JWT.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Stable user id (typically the JWT `sub` claim).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
}

impl Output {
    /// Serialize as one JSON line on stdout under a process-global
    /// mutex (so concurrent writes from different threads don't
    /// interleave). The canonical funnel for every byte the browser
    /// writes. Splices the current [`crate::mode::Mode`] into the
    /// serialized object as a top-level `"mode"` field — `null`
    /// before any mode-setting request has landed.
    pub fn emit(&self) -> std::io::Result<()> {
        let mut value = serde_json::to_value(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        if let serde_json::Value::Object(ref mut map) = value {
            let mode_value = serde_json::to_value(crate::mode::get())
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            map.insert("mode".into(), mode_value);
        }
        let line = value.to_string();
        let _guard = stdout_lock().lock().expect("stdout lock poisoned");
        let mut stdout = std::io::stdout().lock();
        writeln!(stdout, "{line}")?;
        stdout.flush()?;
        Ok(())
    }
}

fn stdout_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}
