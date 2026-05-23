//! Everything the browser writes to stdout.
//!
//! Wire format: one JSON object per line, externally tagged on `"type"`.
//! The browser never prints to stdout or stderr outside [`Output::emit`] —
//! all output (responses, help, errors, diagnostics) flows through here.

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
    /// on every navigation — both native full-page navs (caught by
    /// Tauri's `on_navigation` callback) and SPA route changes
    /// (caught by the injected overlay's `history.pushState` /
    /// `popstate` patch).
    Url { url: String },
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
