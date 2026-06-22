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

use crate::cli::psyops::{PsyOp, PsyopEntry, PublishedPsyop};

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
    /// `psyops get` — full on-disk definition.
    Psyop(PsyOp),
    /// `psyops publish` — what was just committed + resolved
    /// enabled state.
    PublishedPsyop(PublishedPsyop),

    // ── agents ─────────────────────────────────────────────
    /// `agents invite discord` — the bot's server-invite URL.
    DiscordInvite(DiscordInvite),

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

/// Rendered clap text emitted on `--help` / `--version` /
/// missing-subcommand. Wrapping in a struct (rather than a
/// forbidden `Help(String)`) gives consumers a stable name they
/// can route on.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Help {
    pub text: String,
}
