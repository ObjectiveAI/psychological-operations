//! Embedded-Chromium subsystem.
//!
//! The Chrome for Testing zip and the packed extension live inside
//! the Rust binary via `include_bytes!`. On `psychological-operations
//! browse --psyop <name>`:
//!
//!   1. (psyop, commit) are resolved (commit defaults to git HEAD).
//!   2. The chrome zip + extension are content-hash-extracted into
//!      ~/.psychological-operations/chrome/<hash>/ on first run.
//!   3. The native-messaging host is registered (HKCU registry on
//!      Windows, NativeMessagingHosts dir under the profile on
//!      Linux/macOS), pointing at a wrapper script that invokes us
//!      with the `native-host` subcommand.
//!   4. Chromium is spawned with --user-data-dir=<per-psyop profile>,
//!      --load-extension=<extracted ext>, and PSYOP_NAME /
//!      PSYOP_COMMIT_SHA on the env so the eventual native-host
//!      child inherits identity.

pub mod bundles;
pub mod extract;
pub mod launch;
pub mod native_host;
pub mod paths;

use std::fs;

use crate::error::Error;

pub async fn browse(psyop: String, commit: Option<String>) -> Result<crate::Output, Error> {
    let psyop = psyop.trim().to_string();
    if psyop.is_empty() {
        return Err(Error::Other("--psyop is required".into()));
    }
    let commit = match commit {
        Some(c) => c,
        None => derive_commit(&psyop)?,
    };

    let materialized = extract::ensure_extracted()?;
    eprintln!(
        "psychological-operations: chrome materialized at {}",
        materialized.root.display(),
    );

    let profile = paths::profile_dir(&psyop);
    fs::create_dir_all(&profile)?;

    native_host::install(&profile)?;

    launch::spawn(
        &materialized.chrome_binary,
        &materialized.extension_dir,
        &profile,
        &psyop,
        &commit,
    )?;

    Ok(crate::Output::Empty)
}

fn derive_commit(psyop: &str) -> Result<String, Error> {
    let dir = crate::config::psyops_dir().join(psyop);
    let repo = git2::Repository::open(&dir).map_err(|e| {
        Error::Other(format!(
            "PSYOP_COMMIT_SHA unset and git open failed at {}: {e}",
            dir.display(),
        ))
    })?;
    let head = repo.head().and_then(|h| h.peel_to_commit()).map_err(|e| {
        Error::Other(format!("git HEAD lookup failed: {e}"))
    })?;
    Ok(head.id().to_string())
}
