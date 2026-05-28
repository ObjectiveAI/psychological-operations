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
}
