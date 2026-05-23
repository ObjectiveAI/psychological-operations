//! `psychological-operations-browser-sdk` — wire types for the
//! psychological-operations-browser's stdio protocol.
//!
//! Consumed by:
//! - The browser itself (Rust → emits requests to the frontend, receives
//!   responses back from the frontend via a Tauri command).
//! - Any host process that drives the browser via stdin/stdout (e.g.
//!   a future `psychological-operations-cli` integration).
//!
//! ## Event-name convention
//!
//! All Tauri emissions from the browser use the namespace
//! `psyops:<topic>:<event>`. The `psyops` prefix scopes events so
//! consumers can subscribe with confidence the channel isn't used by
//! some other extension or library. Topics (`stdio`, future ones) group
//! related events. Concrete constants live alongside their wire-type
//! definitions so they're the single source of truth.

pub mod stdio;
