use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Target agent for the `agent_queue` destination — selected either by
/// tag or by agent-instance-hierarchy. Untagged: the present field name
/// (`agent_tag` vs `agent_instance_hierarchy`) picks the variant, which
/// also determines the `agent_kind` recorded on each queued row.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum AgentQueue {
    /// `{ "agent_tag": "<tag>" }` — each scored tweet lands in the queue
    /// keyed by this tag.
    AgentTag { agent_tag: String },
    /// `{ "agent_instance_hierarchy": "<hierarchy>" }` — keyed by the
    /// agent-instance-hierarchy, verbatim.
    AgentInstanceHierarchy { agent_instance_hierarchy: String },
}
