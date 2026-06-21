//! Discord bot-creation wizard (browser side) — `Mode::DiscordLogin`.
//!
//! The header shows a read-only auth form (Application ID / Public Key / Bot
//! Token). The content overlay auto-scrapes each value from the portal as the
//! operator navigates and posts it over the `psyops://` scheme as
//! `discord_capture { field, value }` → [`capture_field`], which records it in
//! the in-memory header form. Values can arrive in any order; once **all
//! three** are present the browser emits a single
//! [`Output::DiscordLoginSucceeded`] item carrying them and closes. The CLI
//! persists it — the browser itself never touches the DB.

use psychological_operations_sdk::browser::mode::{self, Mode};
use psychological_operations_sdk::browser::output::Output;
use tauri::{AppHandle, Wry};

fn current_agent() -> Result<String, String> {
    match mode::get() {
        Some(Mode::DiscordLogin { name }) => Ok(name),
        _ => Err("discord capture received outside DiscordLogin mode".into()),
    }
}

/// Record one scraped credential field into the in-memory header form. `field`
/// is one of `application_id` / `public_key` / `bot_token`. When all three are
/// present, emit the single success item for the CLI to persist.
pub async fn capture_field(
    app: &AppHandle<Wry>,
    field: String,
    value: String,
) -> Result<String, String> {
    let name = current_agent()?;
    if !matches!(field.as_str(), "application_id" | "public_key" | "bot_token") {
        return Err(format!("unknown discord field: {field}"));
    }

    // Update the header form (in memory only).
    crate::state::discord_field_set(app, &field, value);

    // Emit once all three values are in hand (order-independent).
    let snap = crate::state::discord_auth_snapshot();
    if let (Some(client_id), Some(public_key), Some(bot_token)) = (
        snap.application_id.value,
        snap.public_key.value,
        snap.bot_token.value,
    ) {
        let _ = Output::DiscordLoginSucceeded {
            client_id,
            public_key,
            bot_token,
        }
        .emit();
    }
    Ok(name)
}
