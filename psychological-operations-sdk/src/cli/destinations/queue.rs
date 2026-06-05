use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Queue {
    /// Target agent name. Each scored tweet lands in this
    /// agent's queue (PRIMARY KEY (agent, tweet_id)).
    pub agent: String,
}
