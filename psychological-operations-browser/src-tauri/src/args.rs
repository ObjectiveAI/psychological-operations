use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use clap::Parser;
use url::Url;

/// Launch a psychological-operations browser session.
///
/// Either `--psyop <name>` or `--x-app` is required. They are mutually
/// supplementary: if both are set, `--x-app` wins.
#[derive(Debug, Parser)]
#[command(name = "psychological-operations-browser")]
#[command(about = "Minimal Tauri webview shell for psych-ops sessions.")]
pub struct Args {
    /// Name of the psyop this session is bound to. Required unless
    /// --x-app is set.
    #[arg(long)]
    pub psyop: Option<String>,

    /// Run the session as the master X-app (root) account.
    #[arg(long)]
    pub x_app: bool,

    /// Base directory for psych-ops state. The browser writes its
    /// webview data (cookies, IndexedDB, cache, etc.) under
    /// <config-base-dir>/plugins/psychological-operations/chromium/
    /// {x-app | psyops/<name>}.
    #[arg(long)]
    pub config_base_dir: PathBuf,

    /// Optional URL to load instead of the default for the session
    /// mode. Defaults: console.x.ai for --x-app, about:blank for --psyop.
    #[arg(long)]
    pub url: Option<Url>,
}

impl Args {
    pub fn validate(&self) -> Result<()> {
        if self.psyop.is_none() && !self.x_app {
            return Err(anyhow!(
                "must pass --psyop <name> or --x-app",
            ));
        }
        Ok(())
    }

    /// Per-session state directory. Matches the path scheme the
    /// (now-dormant) chromium-fork patches used for --user-data-dir,
    /// so any future migration of persisted state is a no-op.
    pub fn target_dir(&self) -> PathBuf {
        let mut p = self.config_base_dir.clone();
        p.push("plugins");
        p.push("psychological-operations");
        p.push("chromium");
        if self.x_app {
            p.push("x-app");
        } else if let Some(name) = &self.psyop {
            p.push("psyops");
            p.push(name);
        }
        p
    }

    /// Resolved initial URL for the session. `--url` wins if set;
    /// otherwise console.x.ai for x-app, about:blank for psyop.
    pub fn initial_url(&self) -> Result<Url> {
        if let Some(u) = &self.url {
            return Ok(u.clone());
        }
        let default = if self.x_app {
            "https://console.x.ai"
        } else {
            "about:blank"
        };
        Url::parse(default).context("internal: failed to parse default URL")
    }
}
