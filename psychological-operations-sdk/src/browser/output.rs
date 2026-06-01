//! Everything the browser writes to stdout.
//!
//! Wire format: one JSON object per line, externally tagged on
//! `"type"`. Mode is locked at the browser's CLI flags for the
//! lifetime of the process; the host knows it without us
//! repeating it on every line. The browser never prints to
//! stdout or stderr outside [`Output::emit`] — all output flows
//! through here.

use std::io::Write;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use super::panel::PanelState;
use super::response::ResponseOutcome;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Output {
    /// A reply to a previously-received [`crate::browser::request::Request`].
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

    /// The URL the active content surface is currently on. Emitted
    /// on the initial overlay mount + every SPA route change
    /// (`history.pushState` / `replaceState` / `popstate` /
    /// `hashchange`) — see `src/overlay/spa-url.ts` — plus
    /// CEF's `DisplayHandler::on_address_change` for full-document
    /// loads.
    Url { url: String },

    /// Sign-in state of the current session. Emitted once on
    /// startup (after the cookies watcher's initial read) and
    /// again every time the auth cookie's presence-or-value
    /// changes. `info` carries identifying claims decoded from
    /// the auth JWT when signed in (currently always `None` for
    /// x.com's `auth_token` which is an opaque session string,
    /// not a JWT — kept on the wire for forward-compat with future
    /// modes whose auth token may carry richer identity).
    SignedIn {
        signed_in: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        info: Option<SignedInInfo>,
    },

    /// Derived panel-condition state. Emitted whenever the state the
    /// instruction panel should show changes — driven by the Rust
    /// `state` module's pure derivation over raw facts (mode, cookies).
    /// See [`crate::browser::panel`].
    Panel { state: PanelState },

    /// A newly-observed tweet ID. Emitted once per ID, in
    /// observation order, by the [`crate::browser::mode::Mode::PsyopRead`]
    /// HTML processor as it dedups against an in-memory ordered
    /// set. The set resets on every mode change (including
    /// psyop swap) so a new session always starts emitting from
    /// zero.
    TweetId { id: String },

    /// Sole terminating signal on the OAuth-success path
    /// ([`crate::browser::mode::Mode::PsyopAuthorize`] /
    /// [`crate::browser::mode::Mode::AgentAuthorize`]). Emitted
    /// once `auth.json` is on disk for the persona; the host
    /// (CLI's `login` command) reads this to know the flow
    /// finished cleanly and sends a `Request::Shutdown` back.
    AuthorizeSucceeded,

    /// Sole terminating signal on the OAuth-failure path.
    /// `error` is the human-readable summary; the operator may
    /// have seen more detail in preceding `Output::Log` entries.
    /// The host reads this to propagate the error and send a
    /// `Request::Shutdown` back.
    AuthorizeFailed { error: String },
}

/// Identifying claims extracted from the auth JWT's payload. All
/// fields are best-effort — the JWT may not carry any of them, in
/// which case the value is `None`. Extensible: adding new claim
/// fields here doesn't break the wire.
///
/// x.com's `auth_token` is opaque (not a JWT), so every field is
/// `None` in the X-App mode today. Kept for forward-compat with
/// future modes whose auth token actually carries identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedInInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
}

impl Output {
    /// Serialize as one JSON line on stdout under a process-global
    /// mutex (so concurrent writes from different threads don't
    /// interleave). The canonical funnel for every byte the browser
    /// writes.
    pub fn emit(&self) -> std::io::Result<()> {
        let line = serde_json::to_string(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
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
