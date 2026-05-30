//! Cross-process-safe per-psyop OAuth refresh-token storage.
//!
//! X rotates the refresh token on every refresh call, so any two
//! processes that both hold the same stored value and race to refresh
//! will invalidate each other (the loser gets a 4xx). The browser and
//! the CLI both want to drive that refresh, so the read-modify-write
//! has to be coordinated across processes. This module is the single
//! seam every caller should go through.
//!
//! Layout (per persona twid, per psyop):
//!
//! ```text
//! <config-base-dir>/plugins/psychological-operations/browser/psyop/<psyop>/handles/<twid>/
//!   ├── auth.json                  (existing — separate from this module)
//!   ├── refresh_token.txt          (raw value; no trailing newline)
//!   └── refresh_token.txt.lock     (empty sentinel; never deleted)
//! ```
//!
//! Locking is via `fs4`'s OS-advisory file locks (`LockFileEx` on
//! Windows, `flock`/`fcntl` on POSIX). Held on the open lock-file
//! handle; released by the kernel when the handle closes, which
//! happens automatically when the [`tokio::fs::File`] is dropped OR
//! when the process exits (including SIGKILL). The lock-sentinel
//! file is created on demand and is **never deleted** — a
//! delete-on-exit cleanup would orphan the lock if the process were
//! killed between unlock and delete.
//!
//! Writes inside the exclusive-lock critical section go through a
//! `refresh_token.txt.tmp` + atomic rename, matching the pattern
//! used in `psychological-operations-browser/src-tauri/src/credentials.rs`.
//!
//! Caveat — `auth.json::refresh_token` is still written by the
//! browser's OAuth flow as part of the larger `Tokens` blob. That
//! shim and this module will be reconciled in a follow-up; until
//! then, callers that round-trip through `set` are the source of
//! truth.
//!
//! Reentrancy — calling [`set`] from inside [`get`] (or vice versa)
//! within the same task will deadlock on Windows (`LockFileEx` is
//! per-handle, not per-process). The public API doesn't compose that
//! way; just don't construct it.

use std::path::{Path, PathBuf};

use fs4::AsyncFileExt;
use tokio::fs;

use crate::cookies::{self, CookiesError};
use crate::mode::Mode;

/// Errors [`get`] and [`set`] can return.
#[derive(Debug)]
pub enum RefreshTokenError {
    /// Resolving the persona twid via the CEF cookies store failed.
    Cookies(CookiesError),
    /// Filesystem I/O (open, lock, read, write, rename).
    Io(std::io::Error),
    /// The `spawn_blocking` task panicked or was cancelled — should
    /// not happen in normal operation.
    Join(tokio::task::JoinError),
    /// CEF has no `twid` cookie for this psyop's profile yet — i.e.,
    /// no persona is signed in. Callers that have already verified
    /// sign-in elsewhere can treat this as a programmer error.
    NoUserSignedIn,
}

impl std::fmt::Display for RefreshTokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cookies(e) => write!(f, "cookies: {e}"),
            Self::Io(e) => write!(f, "io: {e}"),
            Self::Join(e) => write!(f, "spawn_blocking: {e}"),
            Self::NoUserSignedIn => write!(f, "no persona signed in"),
        }
    }
}

impl std::error::Error for RefreshTokenError {}

impl From<std::io::Error> for RefreshTokenError {
    fn from(e: std::io::Error) -> Self { Self::Io(e) }
}

/// Read the refresh token for the persona currently signed into
/// `psyop_name`'s CEF cookie jar. Acquires a shared (multi-reader)
/// filesystem lock for the duration of the read. Returns `Ok(None)`
/// when the file doesn't exist yet (no token has been stored).
pub async fn get(
    config_base_dir: &Path,
    psyop_name: &str,
) -> Result<Option<String>, RefreshTokenError> {
    let dir = persona_dir(config_base_dir, psyop_name).await?;
    let token_path = dir.join("refresh_token.txt");
    let lock_path = dir.join("refresh_token.txt.lock");

    let lock_file = open_lock_file(&lock_path).await?;
    let lock_file = acquire_shared(lock_file).await?;

    let result = match fs::read_to_string(&token_path).await {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(RefreshTokenError::Io(e)),
    };

    // Explicit drop documents intent — the lock is released when the
    // OS closes the underlying fd.
    drop(lock_file);
    result
}

/// Write the refresh token for the persona currently signed into
/// `psyop_name`'s CEF cookie jar. Acquires an exclusive filesystem
/// lock (blocks concurrent readers AND writers across processes)
/// for the duration of the write. The write itself is atomic
/// (temp + rename).
pub async fn set(
    config_base_dir: &Path,
    psyop_name: &str,
    value: &str,
) -> Result<(), RefreshTokenError> {
    let dir = persona_dir(config_base_dir, psyop_name).await?;
    let token_path = dir.join("refresh_token.txt");
    let tmp_path = dir.join("refresh_token.txt.tmp");
    let lock_path = dir.join("refresh_token.txt.lock");

    let lock_file = open_lock_file(&lock_path).await?;
    let lock_file = acquire_exclusive(lock_file).await?;

    fs::write(&tmp_path, value.as_bytes()).await?;
    fs::rename(&tmp_path, &token_path).await?;

    drop(lock_file);
    Ok(())
}

/// Resolve `<config-base-dir>/.../psyop/<psyop>/handles/<twid>/` and
/// ensure it exists. The twid lookup goes through
/// [`cookies::signed_in_x_user_id`] (sync — wrapped in
/// `spawn_blocking` so the rusqlite + DPAPI call doesn't park the
/// async runtime).
async fn persona_dir(
    config_base_dir: &Path,
    psyop_name: &str,
) -> Result<PathBuf, RefreshTokenError> {
    let twid = {
        let base = config_base_dir.to_path_buf();
        let mode = Mode::PsyopAuthorize { name: psyop_name.to_string() };
        tokio::task::spawn_blocking(move || cookies::signed_in_x_user_id(&base, &mode))
            .await
            .map_err(RefreshTokenError::Join)?
            .map_err(RefreshTokenError::Cookies)?
            .ok_or(RefreshTokenError::NoUserSignedIn)?
    };

    let dir = config_base_dir
        .join("plugins")
        .join("psychological-operations")
        .join("browser")
        .join("psyop")
        .join(psyop_name)
        .join("handles")
        .join(&twid);
    fs::create_dir_all(&dir).await?;
    Ok(dir)
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

/// `fs4`'s lock calls are sync syscalls (`flock` / `LockFileEx`) that
/// block until granted. We move the file into `spawn_blocking` for the
/// acquire and hand it back so the lock guard is owned by the async
/// task — drop on return = release.
async fn acquire_shared(file: fs::File) -> Result<fs::File, RefreshTokenError> {
    tokio::task::spawn_blocking(move || {
        AsyncFileExt::lock_shared(&file)?;
        Ok::<_, std::io::Error>(file)
    })
    .await
    .map_err(RefreshTokenError::Join)?
    .map_err(RefreshTokenError::Io)
}

async fn acquire_exclusive(file: fs::File) -> Result<fs::File, RefreshTokenError> {
    tokio::task::spawn_blocking(move || {
        AsyncFileExt::lock(&file)?;
        Ok::<_, std::io::Error>(file)
    })
    .await
    .map_err(RefreshTokenError::Join)?
    .map_err(RefreshTokenError::Io)
}
