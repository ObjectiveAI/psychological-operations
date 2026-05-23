//! CLI arguments + the launch mode they imply.

use std::path::PathBuf;

use clap::Parser;

/// The session mode the browser is running in. Derived from the
/// CLI flags `--x-app` and `--psyop <name>`. Exactly one must be
/// in effect at any time; `--x-app` takes precedence if both are
/// supplied on the command line.
#[allow(dead_code)] // consumed by code added in a later step
#[derive(Debug, Clone)]
pub enum Mode {
    /// Master X-app (root) session.
    XApp,
    /// Per-psyop session, scoped to the named psyop.
    Psyop { name: String },
}

#[derive(Debug, Parser)]
#[command(name = "psychological-operations-browser")]
#[command(about = "Tauri webview shell for psychological-operations sessions.")]
pub struct Args {
    /// Base directory for psych-ops state. Session-local data
    /// (cookies, IndexedDB, cache, ...) is rooted here.
    #[arg(long)]
    pub config_base_dir: PathBuf,

    /// Name of the psyop this session is bound to. Required unless
    /// --x-app is also passed.
    #[arg(long)]
    pub psyop: Option<String>,

    /// Run the session as the master X-app (root) account. When both
    /// --x-app and --psyop are passed, --x-app wins.
    #[arg(long)]
    pub x_app: bool,
}

impl Args {
    /// Resolve the launch mode from the parsed flags. Errors if
    /// neither `--x-app` nor `--psyop` was supplied.
    pub fn mode(&self) -> Result<Mode, &'static str> {
        if self.x_app {
            Ok(Mode::XApp)
        } else if let Some(name) = &self.psyop {
            Ok(Mode::Psyop { name: name.clone() })
        } else {
            Err("must pass --psyop <name> or --x-app")
        }
    }
}
