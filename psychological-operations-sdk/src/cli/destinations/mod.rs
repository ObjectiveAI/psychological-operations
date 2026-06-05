//! The `Destination` enum + per-variant struct definitions —
//! shared body shape for `psychological-operations targets add
//! <selector> '<json>'`. Pure data; the runtime `send_one`
//! dispatcher and per-variant `send()` impls live in the CLI
//! (network/fs/subprocess I/O).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub mod http;
pub mod stdout;
pub mod file;
pub mod exec;
pub mod websocket;
pub mod x;
pub mod queue;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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
    #[serde(rename = "queue")]
    Queue(queue::Queue),
}

/// Returned by `targets deliver` — drain queue counters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliverySummary {
    pub delivered: usize,
    pub failed:    usize,
}
