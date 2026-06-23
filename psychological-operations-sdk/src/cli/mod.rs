//! `psychological-operations-cli` shared output types.
//!
//! Lives in the SDK so other tools that drive the CLI (or want
//! to emit the same wire shape) don't need to depend on the CLI
//! crate. Today: just the `Ok`-side [`Output`] enum.

pub mod hooks;
pub mod output;
pub mod psyops;

pub use output::Output;
