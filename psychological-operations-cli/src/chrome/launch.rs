//! Spawn the embedded Chromium for a psyop. Caller has already
//! resolved (psyop, commit) and ensured the bundle is extracted.

use std::path::Path;
use std::process::Command;

use crate::error::Error;

pub fn spawn(
    chrome_binary: &Path,
    extension_dir: &Path,
    profile: &Path,
    psyop: &str,
    commit: &str,
    landing_url: &str,
) -> Result<(), Error> {
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

    // Spawn detached — we don't wait. The user closes the Chromium
    // window when done; the native host runs as a separate child of
    // Chromium each time the extension connects.
    let child = cmd
        .spawn()
        .map_err(|e| Error::Other(format!("failed to spawn chromium: {e}")))?;

    eprintln!(
        "psychological-operations: spawned chromium (pid {}) for psyop \"{psyop}\" @ {commit}",
        child.id(),
    );
    Ok(())
}
