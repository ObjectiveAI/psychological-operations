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

    // Pre-seed the profile (extension pinned to the toolbar + don't
    // restore previous-session tabs on launch). Idempotent — safe on
    // every spawn.
    crate::chromium::pinned::seed_profile_prefs(&profile, &[extension_id])?;

    let mut cmd = Command::new(&materialized.chromium_binary);
    cmd.arg(format!("--user-data-dir={}", profile.display()));
    cmd.arg(format!("--load-extension={}", materialized.auth_extension_dir.display()));
    cmd.arg(format!("--allowlisted-extension-id={extension_id}"));
    cmd.arg("--no-first-run");
    cmd.arg("--no-default-browser-check");
    cmd.arg("--disable-component-update");
    cmd.arg("--disable-features=ChromeWhatsNewUI,DefaultBrowserPromptRefresh");
    // Land on the X Developer Console root. If X renames this path,
    // the operator can navigate manually — the extension popup form
    // doesn't depend on the URL.
    cmd.arg("https://console.x.com/");

    // No PSYOP_NAME / PSYOP_COMMIT_SHA — the X-App profile isn't a
    // psyop. The auth extension never asks for them.

    let child = cmd.spawn().map_err(|e| {
        Error::Other(format!("failed to spawn chromium for x_app setup: {e}"))
    })?;

    crate::emit::emit(crate::events::Event::XAppSetupInstructions {
        profile: profile.display().to_string(),
        child_pid: child.id(),
        instructions: "\
            - sign into your X account if prompted, then on console.x.com \
              create a Project + App and provision credits.\n\
            - on \"User authentication settings\": set up as a Web App with \
              \"Read and write\" permissions and register \
              `http://127.0.0.1/callback` (host only, no port) as a \
              Callback URI. Required for `psyops oauth <name>`.\n\
            - paste the OAuth 2.0 Client ID + Client Secret (from User \
              authentication settings) and the Bearer Token (from Keys \
              and Tokens) into the extension popup form. Click Save. \
              NOTE: do NOT paste the Consumer Key / Secret Key from the \
              Keys and Tokens page - those are OAuth 1.0a, unused here.\n\
            - this profile persists; future runs reuse the session."
            .to_string(),
    });

    Ok(crate::Output::Empty)
}
