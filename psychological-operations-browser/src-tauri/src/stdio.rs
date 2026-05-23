//! JSON-Lines stdio protocol for the browser.
//!
//! The browser reads one [`request::Request`] per line from stdin
//! and writes one [`response::Response`] per line to stdout. Both
//! enums are externally tagged on a `"type"` field (e.g.
//! `{"type":"html"}` / `{"type":"html","html":"…"}`).
//!
//! This module only defines the wire types. The reader/writer
//! plumbing that drives them is wired up separately.

pub mod request;
pub mod response;

pub use request::Request;
pub use response::Response;
