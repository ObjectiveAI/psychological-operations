//! Twitch API surface — a reqwest-backed Helix (+ OAuth validate) client,
//! authenticated per-agent from the database.
//!
//! Unlike Discord (serenity) and X (codegen'd endpoints), Twitch's surface here
//! is tiny: a whoami/liveness check ([`Client::validate`]), a login→user lookup
//! ([`Client::get_user_by_login`]), and chat send ([`Client::send_message`]).
//! Reads are fronted by the shared response cache (`Db::cache_get_or_fetch` —
//! the same backend the X and Discord clients use); writes go straight through.

mod cache;
pub mod client;
pub mod error;

pub use client::{Client, HelixUser, SentMessage, ValidateResponse};
pub use error::Error;
