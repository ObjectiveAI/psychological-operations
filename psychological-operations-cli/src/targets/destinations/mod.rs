pub mod discord;
pub mod exec;
pub mod file;
pub mod http;
pub mod json_body;
pub mod agent_queue;
pub mod stdout;
pub mod telegram;
pub mod websocket;
pub mod x;

use crate::psyops::PsyOp;
use crate::score::ScoredPost;

// Type definitions moved to the SDK under
// `psychological_operations_sdk::cli::destinations`. Re-export the
// `Destination` enum at this path so call sites keep working.
pub use psychological_operations_sdk::cli::destinations::Destination;

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
/// (for `AuthMode::Psyop`'s OAuth-token loading), but every
/// destination's send takes the same shape for uniformity.
pub async fn send_one(
    dest: &Destination,
    subject: &Subject<'_>,
    ctx: &crate::context::Context,
) -> Result<(), crate::error::Error> {
    match dest {
        Destination::Discord { webhook_url } => discord::send(webhook_url, subject).await,
        Destination::Telegram { bot_token, chat_id } => telegram::send(bot_token, chat_id, subject).await,
        Destination::Http(cfg) => http::send(cfg, subject).await,
        Destination::Stdout(cfg) => stdout::send(cfg, subject).await,
        Destination::File(cfg) => file::send(cfg, subject).await,
        Destination::Exec(cfg) => exec::send(cfg, subject).await,
        Destination::WebSocket(cfg) => websocket::send(cfg, subject).await,
        Destination::X(cfg) => x::send(cfg, subject, ctx).await,
        Destination::AgentQueue(cfg) => agent_queue::send(cfg, subject, ctx).await,
    }
}
