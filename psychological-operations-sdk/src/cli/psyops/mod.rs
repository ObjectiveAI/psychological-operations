//! Psyop type graph. Each psyop *family* lives in its own submodule: [`x`]
//! (tweets) and [`discord`] (Discord messages). The top-level [`Psyop`] enum
//! is the published shape — untagged, discriminated by each family body's
//! `type` field ([`x::PsyopType`] / [`discord::PsyopType`]).
//!
//! Pure data + `validate()` methods. Runtime concerns (db-backed load/save,
//! Python evaluation against live rows) live in the CLI alongside the
//! scoring pipeline.

pub mod discord;
pub mod x;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A published psyop. Untagged: each family body carries a `type`
/// discriminator (`"x"` / `"discord"`) so serde can tell them apart.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Psyop {
    X(x::PsyOp),
    Discord(discord::PsyOp),
}
