//! Per-persona OAuth token types.
//!
//! The actual read / lock / write of the tokens lives on
//! [`crate::x::client::Client`] (`read_auth`, `lock_auth`,
//! `write_auth`) — persisted in the db crate's `auth_tokens` table
//! (keyed by `(kind, name, persona_twid, x_app_twid)`) and coordinated
//! by the postgres advisory locker. This module owns only the shared
//! types: the [`Tokens`] shape stored as the row's JSONB, the
//! [`PersonaKind`] enum that splits psyops from agents, and the
//! staleness buffer everyone agrees on.
//!
//! The X-App twid is the master X dev-account that minted the OAuth
//! credentials used to drive this persona's authorization flow.
//! Swapping the signed-in X-App on console.x.com routes the auth
//! methods to a different `auth_tokens` row under the same persona-twid
//! — each (persona, X-App) pair gets its own independent token store.

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::mode::Mode;

/// Which family of named persona a set of OAuth tokens belongs
/// to. Determines the on-disk root the auth-json APIs read from /
/// write to: `<config>/.../browser/psyop/<name>/handles/<twid>/`
/// vs `<config>/.../browser/agent/<name>/handles/<twid>/`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PersonaKind {
    Psyop,
    Agent,
}

impl PersonaKind {
    /// The TEXT form persisted in the `auth_tokens.kind` column
    /// (and the `psyop`/`agent` discriminant the db crate keys on).
    pub fn db_kind(self) -> &'static str {
        match self {
            PersonaKind::Psyop => "psyop",
            PersonaKind::Agent => "agent",
        }
    }

    /// Map a persona kind + name to the corresponding webview
    /// [`Mode`] (used by the cookies lookup to find the matching
    /// CEF profile).
    pub fn to_mode(self, name: &str) -> Mode {
        match self {
            PersonaKind::Psyop => Mode::PsyopAuthorize { name: name.to_string() },
            PersonaKind::Agent => Mode::AgentAuthorize { name: name.to_string() },
        }
    }
}

/// `access_token` is treated as expired if it lives this much
/// longer or less. Centralised so every consumer of `auth.json`
/// (browser, CLI, future SDK users) agrees on freshness.
pub const FRESHNESS_BUFFER: Duration = Duration::from_secs(30);

/// True iff `tokens.expires_at` is more than [`FRESHNESS_BUFFER`]
/// into the future.
pub fn is_fresh(tokens: &Tokens) -> bool {
    let buffer = chrono::Duration::from_std(FRESHNESS_BUFFER)
        .expect("FRESHNESS_BUFFER fits chrono::Duration");
    tokens.expires_at > Utc::now() + buffer
}

/// OAuth 2.0 token bundle persisted to `auth.json`. The browser's
/// authorize flow mints it; `Http::read_auth` returns it; the
/// CLI (and any other consumer) interprets `expires_at` to decide
/// when to refresh.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Tokens {
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub scope: String,
    pub saved_at: DateTime<Utc>,
}
