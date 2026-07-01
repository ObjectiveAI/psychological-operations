//! Cache-key derivation for the Twitch response cache.
//!
//! The cache backend — storage, lazy TTL-on-read, byte-budget LRU eviction, and
//! the two-tier single-flight lock — is the shared one in the db crate
//! (`Db::cache_get_or_fetch`), the same backend the X and Discord clients use.
//! These helpers only build the 32-byte keys. The `b"twitch\0"` prefix
//! namespaces Twitch keys away from X's `b"cache\0"` and Discord's
//! `b"discord\0"` keys so the schemes can never collide in the shared `cache`
//! table or advisory-lock keyspace.
//!
//! Twitch reads are per-agent (they authenticate as the agent's token, and the
//! app the token belongs to), so the agent `tag` is always folded in. Within a
//! single method the parts are fixed in count and the `method` name
//! disambiguates across methods, so the `\0` separators are unambiguous.

use sha2::{Digest, Sha256};

/// Per-agent key: `SHA-256(b"twitch\0" ‖ tag ‖ \0 ‖ method ‖ (\0 ‖ part)…)`.
pub fn user_key(tag: &str, method: &str, parts: &[&[u8]]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"twitch\0");
    h.update(tag.as_bytes());
    h.update(b"\0");
    h.update(method.as_bytes());
    for p in parts {
        h.update(b"\0");
        h.update(p);
    }
    h.finalize().into()
}
