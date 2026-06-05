use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{json_body, Subject};

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

pub async fn send(cfg: &Http, subject: &Subject<'_>) -> Result<(), crate::error::Error> {
    let body = json_body::build(subject);

    let method = reqwest::Method::from_bytes(cfg.method.as_bytes())
        .map_err(|e| crate::error::Error::Other(format!("invalid http method \"{}\": {e}", cfg.method)))?;

    let client = reqwest::Client::new();
    let mut req = client.request(method, &cfg.url).json(&body);
    for (k, v) in &cfg.headers {
        req = req.header(k, v);
    }

    let res = req.send().await?;
    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        return Err(crate::error::Error::Other(format!(
            "http delivery failed: {status}: {body}",
        )));
    }
    Ok(())
}
