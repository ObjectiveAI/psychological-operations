//! Wire types + helpers for the psychological-operations-browser
//! binary's JSON-Lines stdio protocol.
//!
//! Two top-level wire modules — [`request`] for what the host
//! process sends to the browser, [`response`] for what the
//! browser sends back. Tauri runtime plumbing (event-name
//! constants, the reader thread, the `stdio_respond` command)
//! lives in the browser crate, not here — this module is
//! transport-agnostic.
//!
//! Disk-reading helpers ([`cookies`], [`auth_json`],
//! [`x_app_credentials`]) live here too so external consumers
//! (CLI, future host-side wrappers) can answer questions like
//! "who's signed in?" without going through a running browser.

pub mod auth_json;
pub mod console;
pub mod cookies;
pub mod mode;
pub mod output;
pub mod panel;
pub mod request;
pub mod response;
pub mod x_app_credentials;
