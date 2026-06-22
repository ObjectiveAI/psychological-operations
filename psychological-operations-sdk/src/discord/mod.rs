//! Discord API surface — a serenity-backed client supporting gateway listening
//! and regular REST calls, authenticated per-agent from the database.

pub mod client;
pub mod error;

pub use client::Client;
pub use error::Error;

// Re-export the serenity crate so callers reach `EventHandler`,
// `GatewayIntents`, model types, etc. through the SDK without a direct
// serenity dependency.
pub use serenity;
