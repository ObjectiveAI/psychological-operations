//! Cache-key derivation for the Discord response cache.
//!
//! The cache backend — storage, lazy TTL-on-read, byte-budget LRU eviction, and
//! the two-tier single-flight lock — is the shared one in the db crate
//! (`Db::cache_get_or_fetch`), the same backend the X client uses. These helpers
//! only build the 32-byte keys. The `b"discord\0"` / `b"discord\0user\0"`
//! prefixes namespace Discord keys away from X's `b"cache\0"` keys so the two
//! schemes can never collide in the shared `cache` table or advisory-lock
//! keyspace.
//!
//! Within a single method the parts are fixed in count and width (ids are
//! little-endian, optional cursors carry a presence byte), and the `method`
//! name disambiguates across methods — so the `\0` separators are unambiguous
//! even though a part's bytes may themselves contain `\0`.

use sha2::{Digest, Sha256};

/// Encode an optional snowflake cursor for a cache key: `[0]` for `None`, or
/// `[1] ‖ id_le` for `Some` — a presence byte so `None` and any `Some` value
/// produce distinct, unambiguous key parts.
// Removed once the first paginated (cursor) read method lands.
#[allow(dead_code)]
pub fn opt_cursor(id: Option<u64>) -> Vec<u8> {
    match id {
        Some(x) => {
            let mut v = Vec::with_capacity(9);
            v.push(1);
            v.extend_from_slice(&x.to_le_bytes());
            v
        }
        None => vec![0],
    }
}

/// Global (account-agnostic) key:
/// `SHA-256(b"discord\0" ‖ method ‖ (\0 ‖ part)…)`.
pub fn global_key(method: &str, parts: &[&[u8]]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"discord\0");
    h.update(method.as_bytes());
    for p in parts {
        h.update(b"\0");
        h.update(p);
    }
    h.finalize().into()
}

/// Per-agent key: like [`global_key`] but folds the agent `tag` in under a
/// distinct `b"discord\0user\0"` prefix, so per-bot responses (the guilds it's
/// in, its own identity, permission-filtered channels, its application emojis)
/// never collide across agents sharing the cache.
pub fn user_key(tag: &str, method: &str, parts: &[&[u8]]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"discord\0user\0");
    h.update(tag.as_bytes());
    h.update(b"\0");
    h.update(method.as_bytes());
    for p in parts {
        h.update(b"\0");
        h.update(p);
    }
    h.finalize().into()
}
