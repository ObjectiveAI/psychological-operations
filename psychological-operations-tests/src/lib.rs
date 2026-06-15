//! Cross-crate integration tests for psychological-operations.
//!
//! The real suites will live under `tests/` and exercise the other
//! workspace crates (added here as dev-dependencies as they're needed).
//! For now this is a hello-world scaffold so the crate exists in the
//! workspace and `cargo test -p psychological-operations-tests` is green.

/// Placeholder so the crate has a public surface for the scaffold test.
pub fn hello() -> &'static str {
    "hello, world"
}
