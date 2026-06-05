use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Configuration for an HTTP target destination. Sends a tagged JSON
/// body describing the subject (psyop or read) to an arbitrary endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Http {
    pub url: String,
    #[serde(default = "default_method")]
    pub method: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
}

fn default_method() -> String { "POST".to_string() }
