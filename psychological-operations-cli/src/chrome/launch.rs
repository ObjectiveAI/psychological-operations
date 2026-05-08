//! Spawn the embedded Chromium for a psyop. Caller has already
//! resolved (psyop, commit) and ensured the bundle is extracted.
//!
//! Returns the spawned `Child`. Callers that want detached
//! behavior (e.g., `oauth::setup`) drop the Child; callers that
//! want to block until the operator closes Chromium (e.g.,
//! `psyops browse`) call `child.wait()`.

use std::path::Path;
use std::process::{Child, Command};

use crate::error::Error;

pub fn spawn(
    chrome_binary: &Path,
    extension_dir: &Path,
    profile: &Path,
    psyop: &str,
    commit: &str,
    landing_url: &str,
) -> Result<Child, Error> {
    let extension_id = crate::chrome::bundles::extension_id();

    let mut cmd = Command::new(chrome_binary);
    cmd.arg(format!("--user-data-dir={}", profile.display()));
    cmd.arg(format!("--load-extension={}", extension_dir.display()));
    cmd.arg(format!("--allowlisted-extension-id={extension_id}"));
    cmd.arg("--no-first-run");
    cmd.arg("--no-default-browser-check");
    cmd.arg("--disable-component-update");
    cmd.arg("--disable-features=ChromeWhatsNewUI,DefaultBrowserPromptRefresh");
    cmd.arg(landing_url);

    // Identity threads through the OS-level env; Chromium inherits,
    // and when the extension calls connectNative the host child of
    // Chromium inherits in turn.
    cmd.env("PSYOP_NAME", psyop);
    cmd.env("PSYOP_COMMIT_SHA", commit);

    let child = cmd
        .spawn()
        .map_err(|e| Error::Other(format!("failed to spawn chromium: {e}")))?;

    eprintln!(
        "psychological-operations: spawned chromium (pid {}) for psyop \"{psyop}\" @ {commit}",
        child.id(),
    );
    Ok(child)
}
