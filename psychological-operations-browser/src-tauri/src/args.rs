//! CLI arguments.
//!
//! The browser must be launched with a session mode chosen up
//! front — there's no "blank" CEF browser without a mode because
//! the mode determines which `CefRequestContext` (and therefore
//! which per-account cookie / cache directory) the browser uses.
//! Exactly one of `--x-app`, `--psyop-read <NAME>`, or
//! `--psyop-authorize <NAME>` must be set; multiple or none is
//! a clap error.
//!
//! After startup the host can switch the browser to a different
//! mode by sending a [`psychological_operations_browser_sdk::request::Request::XApp`],
//! [`psychological_operations_browser_sdk::request::Request::PsyopRead`],
//! or [`psychological_operations_browser_sdk::request::Request::PsyopAuthorize`]
//! line on stdin (see [`crate::stdio`]).

use std::path::PathBuf;

use clap::{ArgGroup, Parser};
use psychological_operations_browser_sdk::mode::Mode;

#[derive(Debug, Parser)]
#[command(name = "psychological-operations-browser")]
#[command(about = "Tauri+CEF webview shell for psychological-operations sessions.")]
#[command(group = ArgGroup::new("mode").required(true).multiple(false).args(["x_app", "psyop_read", "psyop_authorize"]))]
pub struct Args {
    /// Base directory for psych-ops state. Mode-specific session
    /// data (cookies, IndexedDB, cache, ...) lives under
    /// `<config-base-dir>/plugins/psychological-operations/browser/cef-root/<mode-subdir>/`.
    /// Credentials live alongside at
    /// `<config-base-dir>/plugins/psychological-operations/browser/<mode-subdir>/`.
    #[arg(long)]
    pub config_base_dir: PathBuf,

    /// Launch in X-App mode. The CEF browser loads
    /// `https://console.x.com/` with a `RequestContext` whose
    /// cache lives at `cef-root/x-app/`.
    #[arg(long, group = "mode")]
    pub x_app: bool,

    /// Launch in Psyop **read** mode, scoped to the given psyop
    /// name. The CEF browser loads `https://x.com/` with a
    /// `RequestContext` whose cache lives at `cef-root/psyop/<name>/`.
    /// The overlay streams page HTML to Rust as the persona
    /// browses; Rust dedups and emits new tweet IDs to stdout.
    #[arg(long, group = "mode", value_name = "NAME")]
    pub psyop_read: Option<String>,

    /// Launch in Psyop **authorize** mode, scoped to the given
    /// psyop name. Same RequestContext as read, but after the
    /// persona signs in Rust drives them through X's OAuth 2.0
    /// PKCE consent screen and writes the resulting tokens to
    /// `<psyop-data-dir>/handles/<persona-twid>/auth.json`.
    #[arg(long, group = "mode", value_name = "NAME")]
    pub psyop_authorize: Option<String>,
}

impl Args {
    /// Resolve the CLI mode flags into the SDK's [`Mode`]. The
    /// `unreachable!` is guarded by clap's required+single group
    /// — clap fails parsing if zero or multiple mode flags are
    /// passed.
    pub fn initial_mode(&self) -> Mode {
        if self.x_app {
            Mode::XApp
        } else if let Some(name) = self.psyop_read.as_ref() {
            Mode::PsyopRead { name: name.clone() }
        } else if let Some(name) = self.psyop_authorize.as_ref() {
            Mode::PsyopAuthorize { name: name.clone() }
        } else {
            unreachable!("clap ArgGroup mode required=true, multiple=false")
        }
    }
}
