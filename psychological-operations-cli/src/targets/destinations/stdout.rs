use serde::{Deserialize, Serialize};

use crate::events::Event;
use super::{json_body, Subject};

/// Marker config for the `stdout` destination. No fields — the
/// PluginOutput JSONL wire is structured JSON regardless, so the
/// destination always emits one `Event::TargetDelivered` per drained
/// delivery with the full `json_body::build(subject)` body.
///
/// Legacy `{"type":"stdout","mode":"…"}` configs deserialize cleanly:
/// serde silently drops the unknown `mode` field.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Stdout {}

pub async fn send(_cfg: &Stdout, subject: &Subject<'_>) -> Result<(), crate::error::Error> {
    let body = json_body::build(subject);
    crate::emit::emit(Event::TargetDelivered {
        body: serde_json::to_value(&body)?,
    });
    Ok(())
}
