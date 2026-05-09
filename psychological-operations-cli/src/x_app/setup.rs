//! `x_app setup` — open chromium against the master X-App profile
//! so the operator can sign into x.com / configure console.x.com /
//! click the extension to save credentials.

use std::process::Command;

use crate::chromium::extract::ensure_extracted;
use crate::chromium::native_host;
use crate::chromium::paths::x_app_profile_dir;
use crate::error::Error;

pub async fn run(cfg: &crate::run::Config) -> Result<crate::Output, Error> {
    let materialized = ensure_extracted(cfg)?;

    let profile = x_app_profile_dir(cfg);
    std::fs::create_dir_all(&profile)?;

    // Same native-host registration the per-psyop browse path uses.
    // The extension on this profile needs the messaging bridge so
    // its "Save credentials" button can ship to x_app.json.
    native_host::install(&profile, cfg)?;

    let extension_id = crate::chromium::bundles::auth_extension_id();

    // Pre-seed the profile so the auth extension shows up pinned to
    // the toolbar on first launch (and stays pinned across re-launches
    // via idempotent merge).
    crate::chromium::pinned::seed_pinned_extensions(&profile, &[extension_id])?;

    let mut cmd = Command::new(&materialized.chromium_binary);
    cmd.arg(format!("--user-data-dir={}", profile.display()));
    cmd.arg(format!("--load-extension={}", materialized.auth_extension_dir.display()));
    cmd.arg(format!("--allowlisted-extension-id={extension_id}"));
    cmd.arg("--no-first-run");
    cmd.arg("--no-default-browser-check");
    cmd.arg("--disable-component-update");
    cmd.arg("--disable-features=ChromeWhatsNewUI,DefaultBrowserPromptRefresh");
    // Land directly on the projects-and-apps page so the operator
    // doesn't have to navigate from the developer-portal home. If X
    // renames this path, the operator can navigate manually — the
    // extension popup form doesn't depend on the URL.
    cmd.arg("https://developer.x.com/en/portal/projects-and-apps");

    // No PSYOP_NAME / PSYOP_COMMIT_SHA — the X-App profile isn't a
    // psyop. The auth extension never asks for them.

    let child = cmd.spawn().map_err(|e| {
        Error::Other(format!("failed to spawn chromium for x_app setup: {e}"))
    })?;

    eprintln!(
        "psychological-operations: spawned chromium (pid {}) for x_app setup",
        child.id(),
    );
    eprintln!("  profile: {}", profile.display());
    eprintln!("  - sign into your X account if prompted, then on developer.x.com");
    eprintln!("    create a Project + App and provision credits.");
    eprintln!("  - on the App's keys-and-tokens page, click the extension toolbar");
    eprintln!("    icon and paste the credentials into the form. Click Save.");
    eprintln!("  - on \"User authentication settings\": enable OAuth 2.0,");
    eprintln!("    select \"Read and write\" permissions, and register");
    eprintln!("    `http://127.0.0.1/callback` (host only, no port) as a");
    eprintln!("    Callback URL. Required for `psyops oauth <name>`.");
    eprintln!("  - this profile persists; future runs reuse the session.");

    Ok(crate::Output::Empty)
}
