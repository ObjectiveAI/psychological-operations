//! Discord daemon hook definitions.
//!
//! A hook is named per agent and stored as JSONB in `discord_hooks`. There are
//! four kinds, internally tagged by `type`: `python` (operator code the daemon
//! runs on every gateway event, raw event JSON as input) plus three
//! *declarative* triggers the daemon evaluates in Rust against `MESSAGE_CREATE`
//! — `mention`, `reply`, and `dm`. On a declarative match the daemon enqueues
//! the triggering message for the agent (with `message` as the note) and
//! notifies it, mirroring `agents enqueue discord`.
//!
//! Each declarative hook has an optional `user_id`; when omitted it defaults to
//! the bot's own Discord user id (resolved by the daemon from the agent's
//! stored `client_id`). That id is both the watch-target (mention/reply) and
//! the author the daemon's self-filter excludes — which is what makes `dm` mean
//! "incoming DMs only" and keeps a hook from firing on the bot's own messages.
//!
//! Pure data + `validate()`. Resolution and matching live in the daemon.

use serde::{Deserialize, Serialize};

/// A Discord daemon hook, internally tagged by `type`
/// (`python` / `mention` / `reply` / `dm`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Hook {
    /// Operator Python, run for every gateway event with the raw event JSON as
    /// input.
    Python { code: String },
    /// Fires when a message `@everyone`s, mentions `user_id`, or mentions a
    /// role that `user_id` holds. `user_id` defaults to the bot's own id.
    Mention {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user_id: Option<String>,
        message: String,
    },
    /// Fires when a message replies to one authored by `user_id` (default: the
    /// bot's own id).
    Reply {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user_id: Option<String>,
        message: String,
    },
    /// Fires on any message added to a DM channel. `user_id` (default: the
    /// bot's own id) is the author the self-filter excludes, so in practice it
    /// means "incoming DMs".
    Dm {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user_id: Option<String>,
        message: String,
    },
}

impl Hook {
    /// The hook's `type` tag (matches the serde discriminator).
    pub fn type_name(&self) -> &'static str {
        match self {
            Hook::Python { .. } => "python",
            Hook::Mention { .. } => "mention",
            Hook::Reply { .. } => "reply",
            Hook::Dm { .. } => "dm",
        }
    }

    /// Insert-time validation. Returns a free-form error string; CLI callers
    /// wrap it with their own error variant.
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Hook::Python { code } => {
                if code.trim().is_empty() {
                    return Err("python hook: code must not be empty".into());
                }
            }
            Hook::Mention { message, .. }
            | Hook::Reply { message, .. }
            | Hook::Dm { message, .. } => {
                if message.trim().is_empty() {
                    return Err("hook message must not be empty".into());
                }
            }
        }
        Ok(())
    }
}

/// A Twitch daemon hook, internally tagged by `type` (`python` / `mention`).
///
/// The daemon evaluates each hook against every chat message it buffers.
/// `python` runs operator code for every message (raw message JSON as input);
/// `mention` is a declarative trigger the daemon evaluates in Rust — on a match
/// it enqueues the triggering message for the agent (with `message` as the note)
/// and notifies it. Twitch has no reply/DM, so those variants don't exist here.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TwitchHook {
    /// Operator Python, run for every chat message with the raw message JSON as
    /// input.
    Python { code: String },
    /// Fires when a chat message `@`-mentions `user_login` (i.e. its text
    /// contains `@<user_login>`, case-insensitive). `user_login` defaults to the
    /// caller's login (baked in at insert), falling back daemon-side to the
    /// hook's own agent login.
    Mention {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user_login: Option<String>,
        message: String,
    },
}

impl TwitchHook {
    /// The hook's `type` tag (matches the serde discriminator).
    pub fn type_name(&self) -> &'static str {
        match self {
            TwitchHook::Python { .. } => "python",
            TwitchHook::Mention { .. } => "mention",
        }
    }

    /// Insert-time validation. Returns a free-form error string; CLI callers
    /// wrap it with their own error variant.
    pub fn validate(&self) -> Result<(), String> {
        match self {
            TwitchHook::Python { code } => {
                if code.trim().is_empty() {
                    return Err("python hook: code must not be empty".into());
                }
            }
            TwitchHook::Mention { message, .. } => {
                if message.trim().is_empty() {
                    return Err("mention hook message must not be empty".into());
                }
            }
        }
        Ok(())
    }
}
