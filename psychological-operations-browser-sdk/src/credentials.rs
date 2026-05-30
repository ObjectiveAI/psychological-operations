//! Wire types for per-handle X-App credential storage.
//!
//! The content webview's overlay observes the user copying each of
//! the five auth values out of the X developer console and ships
//! them to Rust one at a time via the `store_x_app_credential`
//! Tauri command. The Rust side writes each value to a flat
//! `<field>.txt` file under a per-handle directory so callers
//! (today: the browser; tomorrow: the CLI) can `read_to_string`
//! any single field independently. See
//! `psychological-operations-browser/src-tauri/src/credentials.rs`
//! for the storage logic.
//!
//! Identifier alignment is intentional: the snake-case enum name,
//! the on-disk filename, and the CLI's `XAppConfig` field names
//! all match. One identifier, three uses, no adapter layer.
//!
//! Adding a new credential field in the future: add a variant
//! here, add an arm to [`XAppCredentialField::file_name`], wire
//! the corresponding allowlist into the storage module if you
//! need any per-field validation.

use serde::{Deserialize, Serialize};

/// Which X-App credential the `store_x_app_credential` call is
/// setting. Snake-cased on the wire (`client_id`, `consumer_key`,
/// `secret_key`, etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum XAppCredentialField {
    /// OAuth 2.0 client identifier (also referred to as "API Key"
    /// in some X documentation under the OAuth 2 tab). Lives in a
    /// different surface from the post-create dialog — separate
    /// read flow when we get there.
    ClientId,
    /// OAuth 2.0 client secret. Pairs with `ClientId`.
    ClientSecret,
    /// OAuth 2.0 app-only bearer token. Surfaced as
    /// "Bearer Token" in X's post-create dialog.
    BearerToken,
    /// OAuth 1.0a per-user access token. Generated separately
    /// via a user-authorization flow — not in the post-create
    /// dialog.
    AccessToken,
    /// OAuth 1.0a per-user access token secret. Pairs with
    /// `AccessToken` to sign per-user requests.
    AccessTokenSecret,
    /// "Consumer Key" as labeled in X's post-create dialog. The
    /// OAuth 1.0a app-identity public key.
    ConsumerKey,
    /// "Secret Key" as labeled in X's post-create dialog
    /// (X's term for the consumer secret).
    SecretKey,
}

impl XAppCredentialField {
    /// Filename used to store this field's value under the
    /// per-handle directory. The `.txt` files contain exactly the
    /// raw credential string (no JSON envelope, no trailing
    /// newline) so unrelated consumers can read with the minimum
    /// possible deserialization machinery.
    pub fn file_name(self) -> &'static str {
        match self {
            Self::ClientId => "client_id.txt",
            Self::ClientSecret => "client_secret.txt",
            Self::BearerToken => "bearer_token.txt",
            Self::AccessToken => "access_token.txt",
            Self::AccessTokenSecret => "access_token_secret.txt",
            Self::ConsumerKey => "consumer_key.txt",
            Self::SecretKey => "secret_key.txt",
        }
    }
}
