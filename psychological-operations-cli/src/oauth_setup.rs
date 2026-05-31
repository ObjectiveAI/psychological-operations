//! `psyops oauth <name>` — drive the per-psyop OAuth 2.0 PKCE flow.
//!
//! Thin shim: the embedded browser binary runs the entire PKCE dance
//! (cookies watcher detects sign-in, drives consent screen, exchanges
//! the code, writes `<base>/.../psyop/<name>/handles/<twid>/auth.json`
//! via the SDK). The CLI just spawns it and waits.

use crate::browser::{extract::ensure_extracted, launch};
use crate::error::Error;

pub async fn run(psyop_name: &str, cfg: &crate::run::Config) -> Result<crate::Output, Error> {
    let materialized = ensure_extracted(cfg)?;
    let config_base_dir = cfg.objectiveai_base_dir();

    let mut child = launch::spawn(
        &materialized.binary,
        &config_base_dir,
        launch::Mode::PsyopAuthorize { name: psyop_name.to_string() },
        /* pipe_stdout = */ false,
    )?;

    crate::emit::emit(crate::events::Event::BrowserSpawned {
        kind: "psyop_authorize".into(),
        name: Some(psyop_name.to_string()),
        pid: child.id(),
    });

    let status = child.wait().map_err(|e| {
        Error::Other(format!("waiting for browser ({psyop_name}) failed: {e}"))
    })?;

    crate::emit::emit(crate::events::Event::BrowserExit {
        kind: "psyop_authorize".into(),
        name: Some(psyop_name.to_string()),
        status: status.code(),
    });

    Ok(crate::Output::Empty)
}
