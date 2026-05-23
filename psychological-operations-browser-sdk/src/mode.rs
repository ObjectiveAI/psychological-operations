//! The session mode the browser is currently in.
//!
//! Mode is set on the Rust side when a mode-setting [`crate::request::Request`]
//! (e.g. [`crate::request::Request::XApp`]) is processed, and the
//! frontend can query it on each overlay mount via the
//! `current_mode` Tauri command — useful for resuming URL reporting
//! after a full-page navigation has re-mounted the overlay on a new
//! origin.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Mode {
    /// Master X-App (root) session. Frontend lands the user on
    /// `https://console.x.ai/` and reports URL changes from there.
    XApp,
}
