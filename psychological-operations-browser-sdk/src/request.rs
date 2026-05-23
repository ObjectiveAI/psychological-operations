//! Requests the host process sends to the browser.
//!
//! Wire format: one JSON object per line, externally tagged on `"type"`.
//!
//! Examples:
//! ```text
//! {"type":"html"}
//! {"type":"x_app"}
//! ```

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    /// Ask for the active page's serialized outer HTML.
    Html,
    /// Place the browser in X-App (master root) mode. Triggers
    /// creation of the X webview pointed at `https://x.com` with the
    /// X-App data directory. Response is [`crate::response::Response::Ack`].
    XApp,
}
