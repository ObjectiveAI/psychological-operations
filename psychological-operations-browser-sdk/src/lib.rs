//! `psychological-operations-browser-sdk` — wire types for the
//! psychological-operations-browser's JSON-Lines stdio protocol.
//!
//! Two top-level modules — [`request`] for what the host process
//! sends to the browser, [`response`] for what the browser sends
//! back. Tauri runtime plumbing (event-name constants, the reader
//! thread, the `stdio_respond` command) lives in the browser crate,
//! not here — this crate is transport-agnostic.

pub mod console;
pub mod cookies;
pub mod credentials;
pub mod mode;
pub mod output;
pub mod panel;
pub mod refresh_token;
pub mod request;
pub mod response;
