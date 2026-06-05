use std::collections::BTreeMap;

use futures::{SinkExt, StreamExt};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, http::HeaderValue, Message};

use super::{json_body, Subject};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Urls,
    UrlsWithScores,
    Json,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WebSocket {
    pub url: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    pub mode: Mode,
}

pub async fn send(cfg: &WebSocket, subject: &Subject<'_>) -> Result<(), crate::error::Error> {
    let payload = render(&cfg.mode, subject)?;

    let mut request = cfg.url.as_str().into_client_request()
        .map_err(|e| crate::error::Error::Other(format!("websocket invalid url: {e}")))?;
    let req_headers = request.headers_mut();
    for (k, v) in &cfg.headers {
        let value = HeaderValue::from_str(v)
            .map_err(|e| crate::error::Error::Other(format!("websocket invalid header value for \"{k}\": {e}")))?;
        req_headers.insert(
            tokio_tungstenite::tungstenite::http::HeaderName::from_bytes(k.as_bytes())
                .map_err(|e| crate::error::Error::Other(format!("websocket invalid header name \"{k}\": {e}")))?,
            value,
        );
    }

    let (mut stream, _resp) = tokio_tungstenite::connect_async(request).await
        .map_err(|e| crate::error::Error::Other(format!("websocket connect failed: {e}")))?;

    stream.send(Message::Text(payload.into())).await
        .map_err(|e| crate::error::Error::Other(format!("websocket send failed: {e}")))?;
    stream.close(None).await
        .map_err(|e| crate::error::Error::Other(format!("websocket close failed: {e}")))?;

    // Drain remaining frames so the close handshake completes cleanly.
    while let Some(msg) = stream.next().await {
        if msg.is_err() { break; }
    }
    Ok(())
}

fn render(mode: &Mode, subject: &Subject) -> Result<String, crate::error::Error> {
    let mut s = String::new();
    match mode {
        Mode::Urls => {
            let (_, lines) = json_body::lines(subject);
            for (_, url) in lines {
                s.push_str(&url);
                s.push('\n');
            }
        }
        Mode::UrlsWithScores => {
            let (_, lines) = json_body::lines(subject);
            for (label, url) in lines {
                s.push_str(&format!("{label} — {url}\n"));
            }
        }
        Mode::Json => {
            let body = json_body::build(subject);
            s.push_str(&serde_json::to_string(&body)?);
        }
    }
    Ok(s)
}
