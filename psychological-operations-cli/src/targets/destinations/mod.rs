pub mod discord;
pub mod exec;
pub mod file;
pub mod http;
pub mod json_body;
pub mod stderr;
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
    #[serde(rename = "stderr")]
    Stderr(stderr::Stderr),
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

/// Dispatch one destination. `dispatch` calls this in
/// parallel-join + swallows errors; the `targets deliver` path
/// calls it row-by-row + captures errors to bump / delete the
/// queue.
pub async fn send_one(
    dest: &Destination,
    subject: &Subject<'_>,
) -> Result<(), crate::error::Error> {
    match dest {
        Destination::Discord { webhook_url } => discord::send(webhook_url, subject).await,
        Destination::Telegram { bot_token, chat_id } => telegram::send(bot_token, chat_id, subject).await,
        Destination::Http(cfg) => http::send(cfg, subject).await,
        Destination::Stdout(cfg) => stdout::send(cfg, subject).await,
        Destination::Stderr(cfg) => stderr::send(cfg, subject).await,
        Destination::File(cfg) => file::send(cfg, subject).await,
        Destination::Exec(cfg) => exec::send(cfg, subject).await,
        Destination::WebSocket(cfg) => websocket::send(cfg, subject).await,
        Destination::X(cfg) => x::send(cfg, subject).await,
    }
}

pub async fn dispatch(destinations: &[Destination], subject: Subject<'_>) {
    let subject_ref = &subject;
    let futs = destinations.iter().map(|dest| send_one(dest, subject_ref));
    for result in futures::future::join_all(futs).await {
        if let Err(e) = result {
            eprintln!("delivery failed: {e}");
        }
    }
}
