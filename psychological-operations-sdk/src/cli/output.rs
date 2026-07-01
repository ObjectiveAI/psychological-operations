//! Success-side output of a `psychological-operations` CLI
//! invocation. Errors go through a separate channel
//! (`objectiveai_sdk::cli::Error` at the host boundary).
//!
//! Every variant carries a TYPED payload. Opaque
//! `String` / `serde_json::Value` payloads are forbidden — each
//! unique terminal output is its own variant. Variants serialize
//! externally-tagged via `rename_all = "snake_case"`:
//!
//! - `Output::Ok` → `"ok"`
//! - `Output::Schema(s)` → `{"schema": <json-schema>}`
//! - `Output::PsyopList(v)` → `{"psyop_list": [...]}`
//! - etc.

use schemars::Schema;
use serde::{Deserialize, Serialize};

use crate::cli::hooks::Hook;
use crate::cli::psyops::x::{PsyopEntry, PublishedPsyop};
use crate::cli::psyops::PsyOp;

/// Terminal CLI command output. Every variant is typed; no
/// opaque payloads.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Output {
    /// Generic success ack — set / disable / delete / run /
    /// browse / login / setup / queue-handle / mcp-begin, etc.
    Ok,
    /// JSON Schema dump from `psyops schema`.
    Schema(Schema),

    // ── psyops ─────────────────────────────────────────────
    /// `psyops list` — sorted, paginated entries.
    PsyopList(Vec<PsyopEntry>),
    /// `psyops get` — full on-disk definition (either psyop family).
    Psyop(PsyOp),
    /// `psyops publish` — what was just committed + resolved
    /// enabled state.
    PublishedPsyop(PublishedPsyop),

    // ── agents ─────────────────────────────────────────────
    /// `agents invite discord` — the bot's server-invite URL.
    DiscordInvite(DiscordInvite),
    /// `agents daemon discord hooks list` — the agent's hooks
    /// (name + type + description; the definition body is not surfaced).
    DiscordHookList(Vec<DiscordHookEntry>),
    /// `agents daemon discord hooks get` — one hook's full typed definition.
    DiscordHook(DiscordHookFull),
    /// `agents twitch channels list` — the channel logins the daemon JOINs
    /// (and buffers chat from) for an agent.
    TwitchChannelList(Vec<String>),

    // ── meta ───────────────────────────────────────────────
    /// `--help` / `--version` / "missing subcommand" rendered
    /// clap text.
    Help(Help),
}

/// `agents invite discord` — the Discord server-invite URL for an
/// agent's bot (built from the stored client id).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscordInvite {
    pub url: String,
}

/// One `agents daemon discord hooks list` row — a hook's name, type
/// (`python` / `mention` / `reply` / `dm`), and description. The definition
/// body is intentionally omitted from the listing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscordHookEntry {
    pub name: String,
    pub hook_type: String,
    pub description: String,
}

/// `agents daemon discord hooks get` — a hook's name, description, and full
/// typed definition (the SDK `Hook` enum, internally tagged by `type`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscordHookFull {
    pub name: String,
    pub description: String,
    pub definition: Hook,
}

/// Rendered clap text emitted on `--help` / `--version` /
/// missing-subcommand. Wrapping in a struct (rather than a
/// forbidden `Help(String)`) gives consumers a stable name they
/// can route on.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Help {
    pub text: String,
}
