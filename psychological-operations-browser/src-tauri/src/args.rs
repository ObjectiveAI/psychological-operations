//! CLI arguments.
//!
//! The browser has no mode flag — mode (X-App vs Psyop) arrives via
//! stdio as a [`crate::stdio`] request. The only thing the CLI needs
//! to know at process-start time is where to root session state.

use std::path::PathBuf;

use clap::Parser;

#[derive(Debug, Parser)]
#[command(name = "psychological-operations-browser")]
#[command(about = "Tauri webview shell for psychological-operations sessions.")]
pub struct Args {
    /// Base directory for psych-ops state. Mode-specific session data
    /// (cookies, IndexedDB, cache, ...) lives under
    /// `<config-base-dir>/plugins/psychological-operations/browser/<mode>/`.
    #[arg(long)]
    pub config_base_dir: PathBuf,

    /// Override the User-Agent string the X-App content webview
    /// sends. Use a real browser's UA (e.g. Firefox's) when
    /// console.x.com / x.com are fingerprinting WebView2 and
    /// gating login on it. Omit to let WebView2 use its own
    /// default UA — no behavior change for callers who don't
    /// care.
    #[arg(long)]
    pub user_agent: Option<String>,
}
