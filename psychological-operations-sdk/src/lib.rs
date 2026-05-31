//! `psychological-operations-sdk` — shared wire-types + helpers
//! across the psychological-operations system.
//!
//! Today the SDK only carries the [`browser`] module — wire-types
//! for the browser binary's JSON-Lines stdio protocol, plus a few
//! disk-reading helpers used by both the browser and host
//! consumers. Future SDK additions sit alongside `browser` as
//! peer modules.

pub mod browser;
