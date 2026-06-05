use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Marker config for the `stdout` destination. No fields — the
/// PluginOutput JSONL wire is structured JSON regardless, so the
/// destination always emits one `Event::TargetDelivered` per drained
/// delivery with the full `json_body::build(subject)` body.
///
/// Legacy `{"type":"stdout","mode":"…"}` configs deserialize cleanly:
/// serde silently drops the unknown `mode` field.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct Stdout {}
