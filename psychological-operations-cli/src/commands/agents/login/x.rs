//! `agents login x` — sign in an agent's X account (OAuth 2.0 PKCE).
//!
//! Thin wrapper over the shared X authorize flow in [`crate::login`].

/// Run the X authorize flow for agent `name`.
pub async fn run(name: &str, dangerously_reset: bool, ctx: &crate::context::Context) -> bool {
    crate::login::run(
        psychological_operations_sdk::browser::auth_json::PersonaKind::Agent,
        name,
        dangerously_reset,
        ctx,
    )
    .await
}
