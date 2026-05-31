//! Cross-process-safe per-psyop OAuth `auth.json` storage.
//!
//! Holds the full OAuth 2.0 [`Tokens`] bundle (access_token,
//! refresh_token, expires_at, scope, saved_at) the browser's
//! psyop-authorize flow exchanges with X. X rotates the
//! refresh_token on every refresh, so any second process that
//! reads a stale copy and tries to refresh first wins and the
//! loser's refresh_token is invalidated. The browser and the CLI
//! both want to drive that refresh, so the read-modify-write has
//! to be coordinated across processes. This module is the single
//! seam every caller should go through.
//!
//! Layout (per persona twid × per X-App twid, per psyop/agent):
//!
//! ```text
//! <config-base-dir>/plugins/psychological-operations/browser/<kind>/<name>/handles/<persona_twid>/<x_app_twid>/
//!   ├── auth.json                  (serialized `Tokens` blob)
//!   └── auth.json.lock             (empty sentinel; never deleted)
//! ```
//!
//! The X-App twid leaf is the master X dev-account that minted the
//! OAuth credentials used to drive this persona's authorization
//! flow. Swapping the signed-in X-App on console.x.com routes
//! [`get`] / [`get_or_refresh`] (and [`set`]) to a different leaf
//! under the same persona-twid parent, so each (persona, X-App)
//! pair gets its own independent token store.
//!
//! Locking uses `fs4`'s OS-advisory file locks (`LockFileEx` on
//! Windows, `flock` / `fcntl` on POSIX) held on the open
//! lock-file handle; the kernel releases the lock when the
//! handle closes, which happens automatically when the
//! [`tokio::fs::File`] is dropped OR when the process exits
//! (including SIGKILL). The lock-sentinel is created on demand
//! and is **never deleted** — a delete-on-exit cleanup would
//! orphan the lock if the process were killed between unlock
//! and delete.
//!
//! Writes inside the exclusive-lock critical section go through
//! a `auth.json.tmp` + atomic rename.
//!
//! Asymmetric twid resolution:
//!
//!   - [`get`] auto-resolves the persona twid via
//!     [`cookies::signed_in_x_user_id`] — read-side callers want
//!     "the tokens for whoever is currently signed in to this
//!     psyop's cookie jar".
//!   - [`set`] takes an explicit `twid` — the OAuth flow's
//!     tokens belong to a SPECIFIC persona, so auto-resolving
//!     could write under the wrong twid if the user signed out
//!     + back in mid-flow.
//!
//! Reentrancy — calling [`set`] from inside [`get`] (or vice
//! versa) within the same task will deadlock on Windows
//! (`LockFileEx` is per-handle, not per-process). The public API
//! doesn't compose that way; just don't construct it.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use fs4::AsyncFileExt;
use serde::{Deserialize, Serialize};
use tokio::fs;

use super::cookies::{self, CookiesError};
use super::mode::Mode;

/// Which family of named persona a set of OAuth tokens belongs
/// to. Determines the on-disk root the auth-json APIs read from /
/// write to: `<config>/.../browser/psyop/<name>/handles/<twid>/`
/// vs `<config>/.../browser/agent/<name>/handles/<twid>/`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersonaKind {
    Psyop,
    Agent,
}

impl PersonaKind {
    fn dir_segment(self) -> &'static str {
        match self {
            PersonaKind::Psyop => "psyop",
            PersonaKind::Agent => "agent",
        }
    }

    fn to_mode(self, name: &str) -> Mode {
        match self {
            PersonaKind::Psyop => Mode::PsyopAuthorize { name: name.to_string() },
            PersonaKind::Agent => Mode::AgentAuthorize { name: name.to_string() },
        }
    }
}

/// `access_token` is treated as expired if it lives this much
/// longer or less. Centralised so every consumer of `auth.json`
/// (browser, CLI, future SDK users) agrees on freshness.
pub const FRESHNESS_BUFFER: Duration = Duration::from_secs(30);

/// True iff `tokens.expires_at` is more than [`FRESHNESS_BUFFER`]
/// into the future.
pub fn is_fresh(tokens: &Tokens) -> bool {
    let buffer = chrono::Duration::from_std(FRESHNESS_BUFFER)
        .expect("FRESHNESS_BUFFER fits chrono::Duration");
    tokens.expires_at > Utc::now() + buffer
}

/// OAuth 2.0 token bundle persisted to `auth.json`. The browser's
/// authorize flow mints it; the SDK reader returns it; the CLI
/// (and any other consumer) interprets `expires_at` to decide
/// when to refresh.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Tokens {
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub scope: String,
    pub saved_at: DateTime<Utc>,
}

/// Errors [`get`] and [`set`] can return.
#[derive(Debug)]
pub enum AuthJsonError {
    /// Resolving the persona twid via the CEF cookies store failed
    /// (only emitted by [`get`] — [`set`] takes an explicit twid).
    Cookies(CookiesError),
    /// Filesystem I/O (open, lock, read, write, rename).
    Io(std::io::Error),
    /// JSON serialize / deserialize failed.
    Serde(serde_json::Error),
    /// The `spawn_blocking` task panicked or was cancelled — should
    /// not happen in normal operation.
    Join(tokio::task::JoinError),
    /// CEF has no `twid` cookie for this psyop's profile yet — i.e.,
    /// no persona is signed in. Only emitted by [`get`].
    NoUserSignedIn,
}

impl std::fmt::Display for AuthJsonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cookies(e) => write!(f, "cookies: {e}"),
            Self::Io(e) => write!(f, "io: {e}"),
            Self::Serde(e) => write!(f, "json: {e}"),
            Self::Join(e) => write!(f, "spawn_blocking: {e}"),
            Self::NoUserSignedIn => write!(f, "no persona signed in"),
        }
    }
}

impl std::error::Error for AuthJsonError {}

impl From<std::io::Error> for AuthJsonError {
    fn from(e: std::io::Error) -> Self { Self::Io(e) }
}
impl From<serde_json::Error> for AuthJsonError {
    fn from(e: serde_json::Error) -> Self { Self::Serde(e) }
}

/// Read `auth.json` for the persona currently signed into
/// `name`'s CEF cookie jar, under the X-App account currently
/// signed into the X-App CEF profile. Resolves BOTH twids in
/// parallel via [`cookies::signed_in_x_user_id`]. Acquires a
/// shared (multi-reader) filesystem lock for the duration of the
/// read. Returns `Ok(None)` when the file doesn't exist yet (no
/// tokens have been minted for this persona × X-App pair).
pub async fn get(
    config_base_dir: &Path,
    kind: PersonaKind,
    name: &str,
) -> Result<Option<Tokens>, AuthJsonError> {
    let (persona_twid, x_app_twid) = tokio::try_join!(
        resolve_persona_twid(config_base_dir, kind, name),
        resolve_x_app_twid(config_base_dir),
    )?;
    let dir = persona_dir(config_base_dir, kind, name, &persona_twid, &x_app_twid);
    fs::create_dir_all(&dir).await?;
    let auth_path = dir.join("auth.json");
    let lock_path = dir.join("auth.json.lock");

    let lock_file = open_lock_file(&lock_path).await?;
    let lock_file = acquire_shared(lock_file).await?;

    let result = match fs::read(&auth_path).await {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(AuthJsonError::Io(e)),
    };

    // Explicit drop documents intent — the lock is released when the
    // OS closes the underlying fd.
    drop(lock_file);
    result
}

/// Write `auth.json` for an explicit `persona_twid` × `x_app_twid`
/// pair. Acquires an exclusive filesystem lock (blocks concurrent
/// readers AND writers across processes) for the duration of the
/// write. The write itself is atomic (temp + rename).
///
/// Both twids are explicit (not auto-resolved): the OAuth flow's
/// freshly-minted tokens always land under the persona they were
/// minted for AND the X-App account that minted them, even if the
/// user signs out + back in on either profile mid-flow.
pub async fn set(
    config_base_dir: &Path,
    kind: PersonaKind,
    name: &str,
    persona_twid: &str,
    x_app_twid: &str,
    tokens: &Tokens,
) -> Result<(), AuthJsonError> {
    let dir = persona_dir(config_base_dir, kind, name, persona_twid, x_app_twid);
    fs::create_dir_all(&dir).await?;
    let auth_path = dir.join("auth.json");
    let tmp_path = dir.join("auth.json.tmp");
    let lock_path = dir.join("auth.json.lock");

    let lock_file = open_lock_file(&lock_path).await?;
    let lock_file = acquire_exclusive(lock_file).await?;

    let mut json = serde_json::to_vec_pretty(tokens)?;
    json.push(b'\n');
    fs::write(&tmp_path, &json).await?;
    fs::rename(&tmp_path, &auth_path).await?;

    drop(lock_file);
    Ok(())
}

/// Pure path resolver for callers that don't need the lock — e.g.
/// the cross-psyop conflict walk that just wants a per-sibling
/// `try_exists`. No I/O; no directory creation.
pub fn path_for(
    config_base_dir: &Path,
    kind: PersonaKind,
    name: &str,
    persona_twid: &str,
    x_app_twid: &str,
) -> PathBuf {
    persona_dir(config_base_dir, kind, name, persona_twid, x_app_twid).join("auth.json")
}

/// Read `auth.json` for the persona currently signed into
/// `psyop_name`'s CEF cookie jar; if the `access_token` expires
/// within the next [`FRESHNESS_BUFFER`], call `refresh` (the
/// caller-supplied closure that POSTs to X's token endpoint) and
/// persist the refreshed tokens atomically.
///
/// Coordination is shared-then-exclusive: a shared lock guards
/// the optimistic read; if the tokens are already fresh, the
/// lock drops immediately. Only on staleness does the function
/// re-acquire as exclusive, re-read (in case a concurrent
/// process refreshed in the gap), and run the refresh closure
/// + atomic write under the exclusive lock.
///
/// `Ok` always carries fresh tokens. `Err(AuthJsonError::Io)`
/// with kind `NotFound`-shaped diagnostics if no `auth.json` is
/// on disk to refresh against.
pub async fn get_or_refresh<F, Fut>(
    config_base_dir: &Path,
    kind: PersonaKind,
    name: &str,
    refresh: F,
) -> Result<Tokens, AuthJsonError>
where
    F: FnOnce(Tokens) -> Fut,
    Fut: Future<Output = Result<Tokens, AuthJsonError>>,
{
    let (persona_twid, x_app_twid) = tokio::try_join!(
        resolve_persona_twid(config_base_dir, kind, name),
        resolve_x_app_twid(config_base_dir),
    )?;
    let dir = persona_dir(config_base_dir, kind, name, &persona_twid, &x_app_twid);
    fs::create_dir_all(&dir).await?;
    let auth_path = dir.join("auth.json");
    let tmp_path = dir.join("auth.json.tmp");
    let lock_path = dir.join("auth.json.lock");

    // Phase 1 — optimistic shared-locked read.
    {
        let lock_file = open_lock_file(&lock_path).await?;
        let lock_file = acquire_shared(lock_file).await?;
        let snapshot = read_auth_json(&auth_path).await?;
        drop(lock_file);
        if let Some(tokens) = &snapshot {
            if is_fresh(tokens) {
                return Ok(snapshot.unwrap());
            }
        }
    }

    // Phase 2 — exclusive lock, re-read (someone else may have
    // refreshed while we waited), refresh-if-needed, write.
    let lock_file = open_lock_file(&lock_path).await?;
    let lock_file = acquire_exclusive(lock_file).await?;

    let existing = read_auth_json(&auth_path).await?;
    let stale = match existing {
        Some(t) if is_fresh(&t) => {
            drop(lock_file);
            return Ok(t);
        }
        Some(t) => t,
        None => {
            drop(lock_file);
            return Err(AuthJsonError::Io(std::io::Error::other(
                "no auth.json on disk to refresh against",
            )));
        }
    };

    let refreshed = refresh(stale).await?;
    let mut json = serde_json::to_vec_pretty(&refreshed)?;
    json.push(b'\n');
    fs::write(&tmp_path, &json).await?;
    fs::rename(&tmp_path, &auth_path).await?;
    drop(lock_file);
    Ok(refreshed)
}

async fn read_auth_json(path: &Path) -> Result<Option<Tokens>, AuthJsonError> {
    match fs::read(path).await {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(AuthJsonError::Io(e)),
    }
}

// ----- internal -------------------------------------------------

fn persona_dir(
    config_base_dir: &Path,
    kind: PersonaKind,
    name: &str,
    persona_twid: &str,
    x_app_twid: &str,
) -> PathBuf {
    config_base_dir
        .join("plugins")
        .join("psychological-operations")
        .join("browser")
        .join(kind.dir_segment())
        .join(name)
        .join("handles")
        .join(persona_twid)
        .join(x_app_twid)
}

/// Look up the persona twid via [`cookies::signed_in_x_user_id`]
/// against the per-psyop / per-agent CEF profile. Sync (rusqlite +
/// DPAPI) — wrapped in `spawn_blocking` so it doesn't park the
/// async runtime.
async fn resolve_persona_twid(
    config_base_dir: &Path,
    kind: PersonaKind,
    name: &str,
) -> Result<String, AuthJsonError> {
    let base = config_base_dir.to_path_buf();
    let mode = kind.to_mode(name);
    tokio::task::spawn_blocking(move || cookies::signed_in_x_user_id(&base, &mode))
        .await
        .map_err(AuthJsonError::Join)?
        .map_err(AuthJsonError::Cookies)?
        .ok_or(AuthJsonError::NoUserSignedIn)
}

/// Look up the X-App master account twid via
/// [`cookies::signed_in_x_user_id`] against the X-App CEF profile.
/// Determines which `<x_app_twid>` leaf the auth.json reader /
/// writer targets.
async fn resolve_x_app_twid(
    config_base_dir: &Path,
) -> Result<String, AuthJsonError> {
    let base = config_base_dir.to_path_buf();
    let mode = Mode::XApp;
    tokio::task::spawn_blocking(move || cookies::signed_in_x_user_id(&base, &mode))
        .await
        .map_err(AuthJsonError::Join)?
        .map_err(AuthJsonError::Cookies)?
        .ok_or(AuthJsonError::NoUserSignedIn)
}

/// Open (creating if absent) the lock-sentinel file in read+write
/// mode so `LockFileEx` on Windows accepts it for both shared and
/// exclusive locks.
async fn open_lock_file(path: &Path) -> std::io::Result<fs::File> {
    fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)
        .await
}

/// `fs4`'s lock calls are sync syscalls (`flock` / `LockFileEx`)
/// that block until granted. Move the file into `spawn_blocking`
/// for the acquire and hand it back so the lock guard is owned
/// by the async task — drop on return = release.
async fn acquire_shared(file: fs::File) -> Result<fs::File, AuthJsonError> {
    tokio::task::spawn_blocking(move || {
        AsyncFileExt::lock_shared(&file)?;
        Ok::<_, std::io::Error>(file)
    })
    .await
    .map_err(AuthJsonError::Join)?
    .map_err(AuthJsonError::Io)
}

async fn acquire_exclusive(file: fs::File) -> Result<fs::File, AuthJsonError> {
    tokio::task::spawn_blocking(move || {
        AsyncFileExt::lock(&file)?;
        Ok::<_, std::io::Error>(file)
    })
    .await
    .map_err(AuthJsonError::Join)?
    .map_err(AuthJsonError::Io)
}
