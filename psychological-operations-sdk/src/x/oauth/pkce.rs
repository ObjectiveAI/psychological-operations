//! PKCE pair + state nonce generation for the OAuth 2.0 PKCE flow.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use sha2::{Digest, Sha256};

pub struct Pkce {
    pub code_verifier:  String,
    pub code_challenge: String,
}

/// Generate a fresh PKCE pair. The verifier is 64 url-safe base64
/// chars (well within the spec's 43–128 range); the challenge is
/// `base64url(SHA256(verifier))` per RFC 7636 §4.2.
pub fn generate() -> Pkce {
    let mut bytes = [0u8; 48];
    rand::thread_rng().fill_bytes(&mut bytes);
    let code_verifier = URL_SAFE_NO_PAD.encode(bytes);

    let digest = Sha256::digest(code_verifier.as_bytes());
    let code_challenge = URL_SAFE_NO_PAD.encode(digest);

    Pkce { code_verifier, code_challenge }
}

/// Generate a fresh state nonce — 32 url-safe base64 chars.
pub fn random_state() -> String {
    let mut bytes = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}
