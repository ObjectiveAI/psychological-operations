//! Psyop type graph. Each psyop *family* lives in its own submodule; today
//! the only family is [`x`] (tweets). The top-level [`Psyop`] enum is the
//! published shape — untagged, discriminated by each family body's `type`
//! field ([`x::PsyopType`]).
//!
//! Pure data + `validate()` methods. Runtime concerns (db-backed load/save,
//! Python evaluation against live `Tweet` rows) live in the CLI alongside the
//! scoring pipeline.

pub mod x;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A published psyop. Untagged: each family body carries a `type`
/// discriminator ([`x::PsyopType`]) so serde can tell them apart. Only the
/// `X` family exists today.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Psyop {
    X(x::PsyOp),
}
