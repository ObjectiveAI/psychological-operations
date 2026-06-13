//! Cache-key derivation for the X v2 API response cache.
//!
//! The cache itself (storage, LRU eviction, the thundering-herd lock)
//! lives in the db crate (`psychological_operations_db::Db::cache_*`).
//! These two key helpers stay in the SDK because they need
//! `reqwest::Method` and the request shape; the [`Client`] hands the
//! resulting 32-byte key to `Db::cache_get_or_fetch`.
//!
//! [`Client`]: super::client::Client

use reqwest::Method;
use sha2::{Digest, Sha256};

/// `SHA-256("cache\0" ‖ method ‖ \0 ‖ path ‖ \0 ‖ query ‖ \0 ‖ body)`.
/// The `cache\0` prefix namespaces away from the auth locker's keys
/// (`"auth\0" ‖ …`) so they can never collide in the advisory-lock
/// keyspace.
pub fn request_key(method: &Method, path: &str, query: &[u8], body: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"cache\0");
    h.update(method.as_str().as_bytes());
    h.update(b"\0");
    h.update(path.as_bytes());
    h.update(b"\0");
    h.update(query);
    h.update(b"\0");
    h.update(body);
    h.finalize().into()
}

/// Like [`request_key`] but folds the authenticated twid into the
/// digest. For endpoints whose response varies by authenticated user
/// (today: `/2/users/me`). The distinct `"cache\0auth\0"` prefix
/// namespaces these keys away from the unscoped [`request_key`] scheme
/// so identical (method, path, query, body) tuples can't collide across
/// the two key spaces.
pub fn request_key_auth_scoped(
    twid: &str,
    method: &Method,
    path: &str,
    query: &[u8],
    body: &[u8],
) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"cache\0auth\0");
    h.update(twid.as_bytes());
    h.update(b"\0");
    h.update(method.as_str().as_bytes());
    h.update(b"\0");
    h.update(path.as_bytes());
    h.update(b"\0");
    h.update(query);
    h.update(b"\0");
    h.update(body);
    h.finalize().into()
}
