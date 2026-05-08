//! Resolve `(psyop, commit)` for the current native-host invocation.
//!
//! Lookup order:
//!   1. `PSYOP_NAME`           — required.
//!   2. `PSYOP_COMMIT_SHA`     — optional; if set, used verbatim.
//!   3. `git rev-parse HEAD`   — run inside `<psyops_dir>/<PSYOP_NAME>/`
//!                               to derive the commit when env didn't
//!                               supply one.
//!
//! Each Chrome profile is launched with `PSYOP_NAME` (and optionally
//! `PSYOP_COMMIT_SHA`) set; Chrome inherits the env and propagates it
//! to the native-messaging child process. That's the entire identity
//! threading mechanism — no per-profile config files are read.

#[derive(Debug, Clone)]
pub struct Identity {
    pub psyop: String,
    pub commit: String,
}

pub fn resolve(cfg: &crate::run::Config) -> Result<Identity, crate::error::Error> {
    let psyop = std::env::var("PSYOP_NAME").map_err(|_| {
        crate::error::Error::Other("PSYOP_NAME is not set".into())
    })?;
    if psyop.trim().is_empty() {
        return Err(crate::error::Error::Other(
            "PSYOP_NAME is empty".into(),
        ));
    }

    if let Ok(sha) = std::env::var("PSYOP_COMMIT_SHA") {
        if !sha.trim().is_empty() {
            return Ok(Identity { psyop, commit: sha });
        }
    }

    // Fall back to the git HEAD of <psyops_dir>/<psyop>/.
    let dir = crate::config::psyops_dir(cfg).join(&psyop);
    let repo = git2::Repository::open(&dir).map_err(|e| {
        crate::error::Error::Other(format!(
            "PSYOP_COMMIT_SHA unset and git open failed at {}: {e}",
            dir.display(),
        ))
    })?;
    let head = repo.head().and_then(|h| h.peel_to_commit()).map_err(|e| {
        crate::error::Error::Other(format!("git HEAD lookup failed: {e}"))
    })?;
    Ok(Identity {
        psyop,
        commit: head.id().to_string(),
    })
}
