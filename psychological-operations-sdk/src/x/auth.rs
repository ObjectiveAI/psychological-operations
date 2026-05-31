//! Auth-file coordination types — [`PersonaKey`] identifies one
//! persona × X-App's `auth.json`; [`AuthLock`] is the opaque
//! two-tier lock that [`super::client::Client::lock_auth`] returns
//! and [`super::client::Client::write_auth`] consumes.
//!
//! `AuthLock` has no public constructor — its only field is
//! private and its only producer is `Http::lock_auth`. External
//! crates can hold one (e.g. browser tauri after acquiring it
//! through `Http`) and pass it back to `Http::write_auth`, but
//! they can't synthesize one to bypass the lock.

use sha2::{Digest, Sha256};

use crate::browser::auth_json::PersonaKind;

use super::locker::LockGuard;

/// Identifies one `auth.json` file: a specific persona
/// (kind + name + persona twid) under a specific X-App account.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PersonaKey {
    pub kind:         PersonaKind,
    pub name:         String,
    /// The X-user-id of the persona signed into this profile's
    /// CEF cookie jar.
    pub persona_twid: String,
    /// The X-user-id of the X-App master account that minted the
    /// OAuth credentials for this persona.
    pub x_app_twid:   String,
}

/// Opaque handle to a held auth lock. Acquired via
/// [`super::client::Client::lock_auth`] and consumed by
/// [`super::client::Client::write_auth`] (or dropped to release
/// without writing). The contained guard is `pub(crate)`, so
/// external code cannot construct an `AuthLock` to fake holding
/// the lock.
pub struct AuthLock {
    pub(crate) guard:   LockGuard,
    pub(crate) persona: PersonaKey,
}

impl AuthLock {
    /// `pub(crate)` — only the `Http::lock_auth` path can build one.
    pub(crate) fn new(guard: LockGuard, persona: PersonaKey) -> Self {
        Self { guard, persona }
    }

    /// The persona this lock was acquired for. `Http::write_auth`
    /// uses this to compute the path it writes to — the caller
    /// can't redirect the write to a different persona than they
    /// locked.
    pub fn persona(&self) -> &PersonaKey {
        &self.persona
    }
}

/// `SHA-256("auth\0" ‖ kind_byte ‖ \0 ‖ name ‖ \0 ‖ persona_twid ‖ \0 ‖ x_app_twid)`.
/// Namespace-prefixed with `"auth\0"` so it can share the `locks`
/// table with the response cache (whose keys start with `"cache\0"`).
pub(crate) fn auth_lock_key(p: &PersonaKey) -> [u8; 32] {
    let kind_byte: u8 = match p.kind {
        PersonaKind::Psyop => b'p',
        PersonaKind::Agent => b'a',
    };
    let mut h = Sha256::new();
    h.update(b"auth\0");
    h.update([kind_byte]);
    h.update(b"\0");
    h.update(p.name.as_bytes());
    h.update(b"\0");
    h.update(p.persona_twid.as_bytes());
    h.update(b"\0");
    h.update(p.x_app_twid.as_bytes());
    h.finalize().into()
}
