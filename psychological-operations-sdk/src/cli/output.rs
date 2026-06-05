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

use objectiveai_sdk::cli::command::functions::inventions::recursive::create::remote::ResponseItem as InventionResponseItem;
use schemars::Schema;
use serde::{Deserialize, Serialize};

use crate::cli::destinations::{Destination, DeliverySummary};
use crate::cli::psyops::{PsyOp, PsyopEntry, PublishedPsyop};

/// Terminal CLI command output. Every variant is typed; no
/// opaque payloads.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Output {
    /// Generic success ack — set / disable / delete / run /
    /// browse / login / setup / queue-handle / mcp-begin /
    /// invention-streaming, etc.
    Ok,
    /// JSON Schema dump from `psyops schema` / `targets schema`.
    Schema(Schema),

    // ── psyops ─────────────────────────────────────────────
    /// `psyops list` — sorted, paginated entries.
    PsyopList(Vec<PsyopEntry>),
    /// `psyops get` — full on-disk definition.
    Psyop(PsyOp),
    /// `psyops publish` — what was just committed + resolved
    /// enabled state.
    PublishedPsyop(PublishedPsyop),

    // ── targets ────────────────────────────────────────────
    /// `targets list` — paginated destination entries.
    DestinationList(Vec<Destination>),
    /// `targets deliver` — drain summary.
    DeliverySummary(DeliverySummary),

    // ── functions ──────────────────────────────────────────
    /// `functions invent {alpha-scalar|alpha-vector|remote}` —
    /// each emission carries the objectiveai SDK's
    /// `recursive::create::remote::ResponseItem` verbatim
    /// (`Chunk` for streaming items, `Id` for the unary terminal
    /// id). The CLI emits one `Output::Invention` per item.
    Invention(InventionResponseItem),

    // ── meta ───────────────────────────────────────────────
    /// `--help` / `--version` / "missing subcommand" rendered
    /// clap text.
    Help(Help),
}

/// Rendered clap text emitted on `--help` / `--version` /
/// missing-subcommand. Wrapping in a struct (rather than a
/// forbidden `Help(String)`) gives consumers a stable name they
/// can route on.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Help {
    pub text: String,
}
