//! Twitch master app-setup wizard (browser side) — `Mode::TwitchApp`.
//!
//! The content overlay read-only-scrapes the Twitch developer console for the
//! app's `client_id` and `client_secret` as the operator navigates and posts
//! each over the `psyops://` scheme as `twitch_capture { field, value }` →
//! [`capture_field`], which records it in the in-memory form. Values can arrive
//! in any order (the secret is only shown once, after "New Secret"); once
//! **both** are present the browser emits a single
//! [`Output::TwitchAppSetupSucceeded`] item carrying them and closes. The CLI
//! persists it — the browser itself never touches the DB.

use psychological_operations_sdk::browser::mode::{self, Mode};
use psychological_operations_sdk::browser::output::Output;
use tauri::{AppHandle, Wry};

fn ensure_twitch_app_mode() -> Result<(), String> {
    match mode::get() {
        Some(Mode::TwitchApp) => Ok(()),
        _ => Err("twitch capture received outside TwitchApp mode".into()),
    }
}

/// Record one scraped credential field into the in-memory form. `field` is one
/// of `client_id` / `client_secret`. When both are present, emit the single
/// success item for the CLI to persist.
pub async fn capture_field(
    app: &AppHandle<Wry>,
    field: String,
    value: String,
) -> Result<(), String> {
    ensure_twitch_app_mode()?;
    if !matches!(field.as_str(), "client_id" | "client_secret") {
        return Err(format!("unknown twitch field: {field}"));
    }

    // Update the in-memory form (no DB write).
    crate::state::twitch_field_set(app, &field, value);

    // Emit once both values are in hand (order-independent).
    let snap = crate::state::twitch_app_snapshot();
    if let (Some(client_id), Some(client_secret)) = (snap.client_id, snap.client_secret) {
        let _ = Output::TwitchAppSetupSucceeded {
            client_id,
            client_secret,
        }
        .emit();
    }
    Ok(())
}
