pub mod discord;
pub mod exec;
pub mod file;
pub mod http;
pub mod json_body;
pub mod stdout;
pub mod telegram;
pub mod websocket;
pub mod x;

use serde::{Deserialize, Serialize};

use crate::psyops::PsyOp;
use crate::score::ScoredPost;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Destination {
    #[serde(rename = "discord")]
    Discord { webhook_url: String },
    #[serde(rename = "telegram")]
    Telegram { bot_token: String, chat_id: String },
    #[serde(rename = "http")]
    Http(http::Http),
    #[serde(rename = "stdout")]
    Stdout(stdout::Stdout),
    #[serde(rename = "file")]
    File(file::File),
    #[serde(rename = "exec")]
    Exec(exec::Exec),
    #[serde(rename = "websocket")]
    WebSocket(websocket::WebSocket),
    #[serde(rename = "x")]
    X(x::X),
}

/// What's being delivered. Text-mode renderers print a per-tweet
/// line list; JSON-mode renderers emit a tagged Body via
/// `json_body::build`. The X destination consumes the post IDs to
/// like / retweet on the platform.
pub enum Subject<'a> {
    Psyop {
        name: &'a str,
        psyop: &'a PsyOp,
        output: &'a [&'a ScoredPost],
    },
}

/// Dispatch one destination. Used by `targets::drain_queue`
/// row-by-row, capturing errors to bump / delete the queued row.
/// `rt` is the runtime config — only the X destination needs it
/// (for `Http::for_psyop`'s OAuth-token loading), but every
/// destination's send takes the same shape for uniformity.
pub async fn send_one(
    dest: &Destination,
    subject: &Subject<'_>,
    rt: &crate::run::Config,
) -> Result<(), crate::error::Error> {
    match dest {
        Destination::Discord { webhook_url } => discord::send(webhook_url, subject).await,
        Destination::Telegram { bot_token, chat_id } => telegram::send(bot_token, chat_id, subject).await,
        Destination::Http(cfg) => http::send(cfg, subject).await,
        Destination::Stdout(cfg) => stdout::send(cfg, subject).await,
        Destination::File(cfg) => file::send(cfg, subject).await,
        Destination::Exec(cfg) => exec::send(cfg, subject).await,
        Destination::WebSocket(cfg) => websocket::send(cfg, subject).await,
        Destination::X(cfg) => x::send(cfg, subject, rt).await,
    }
}
