//! Discord bot-creation wizard (browser side) — `Mode::DiscordLogin`.
//!
//! The CEF content overlay (`discord-login-helpers`, gated on the locked
//! mode) drives the operator through the Discord developer portal: sign in,
//! create the application, add a bot, reveal/reset its token. Once it has
//! the token it posts it back over the `psyops://` scheme
//! (`discord_bot_token`), which lands in [`store_bot_token`] — we persist it
//! for the agent and emit the [`Output::DiscordLoginSucceeded`] terminator
//! the CLI's `agents login discord` is waiting on.
//!
//! The portal navigation + token-scrape DOM walk live in the overlay and
//! are the iteration points; this module is the Rust-side landing.

use psychological_operations_db::Db;
use psychological_operations_sdk::browser::mode::{self, Mode};
use psychological_operations_sdk::browser::output::Output;
use tauri::{AppHandle, Manager, Wry};

/// Persist the scraped bot token for the current `DiscordLogin` agent and
/// fire the success terminator. Returns the agent tag it was stored under.
pub async fn store_bot_token(app: &AppHandle<Wry>, token: String) -> Result<String, String> {
    let name = match mode::get() {
        Some(Mode::DiscordLogin { name }) => name,
        _ => return Err("discord_bot_token received outside DiscordLogin mode".into()),
    };

    let db = app.state::<Db>();
    db.discord_auth_set(&name, &token)
        .await
        .map_err(|e| format!("store discord bot token for {name}: {e}"))?;

    let _ = Output::Log {
        message: format!("discord: stored bot token for {name}"),
    }
    .emit();
    let _ = Output::DiscordLoginSucceeded.emit();
    Ok(name)
}
