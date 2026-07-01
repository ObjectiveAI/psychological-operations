//! CLI arguments.
//!
//! The browser must be launched with a session mode chosen up
//! front — there's no "blank" CEF browser without a mode because
//! the mode determines which `CefRequestContext` (and therefore
//! which per-account cookie / cache directory) the browser uses.
//! Exactly one of `--x-app`, `--agent-read <TAG>`,
//! `--agent-authorize <TAG>`, `--agent-browser <TAG>`, or
//! `--agent-deliver <TAG>` (with `--items <JSON>`) must be set;
//! multiple or none is a clap error.
//!
//! Mode is locked at process startup — there is no runtime
//! mode swap. To change mode, kill the browser and relaunch
//! with a different flag.

use std::path::PathBuf;

use clap::{ArgGroup, Parser};
use psychological_operations_sdk::browser::mode::Mode;

#[derive(Debug, Parser)]
#[command(name = "psychological-operations-browser")]
#[command(about = "Tauri+CEF webview shell for psychological-operations sessions.")]
#[command(group = ArgGroup::new("mode").required(true).multiple(false).args(["x_app", "agent_read", "agent_authorize", "agent_browser", "agent_deliver", "discord_login", "twitch_app", "twitch_authorize"]))]
pub struct Args {
    /// Base directory for psych-ops state. Mode-specific CEF session
    /// data (cookies, IndexedDB, cache, ...) lives under
    /// `<state-dir>/browser/cef-root/<mode-subdir>/`. The browser does
    /// NOT persist anything itself — each mode emits its captured data on
    /// stdout and the CLI writes it to the DB.
    #[arg(long)]
    pub state_dir: PathBuf,

    /// Launch in X-App mode. The CEF browser loads
    /// `https://console.x.com/` with a `RequestContext` whose
    /// cache lives at `cef-root/x-app/`.
    #[arg(long, group = "mode")]
    pub x_app: bool,

    /// Launch in Agent **read** mode, scoped to the given agent
    /// tag. The CEF browser loads `https://x.com/` with a
    /// `RequestContext` whose cache lives at `cef-root/agent-<tag>/`.
    /// The overlay streams page HTML to Rust as the agent browses
    /// its For You feed; Rust dedups and emits new tweet IDs to
    /// stdout. This is the for-you collection mode for `psyops run`.
    #[arg(long, group = "mode", value_name = "TAG")]
    pub agent_read: Option<String>,

    /// Launch in Agent **authorize** mode, scoped to the given
    /// agent tag. Same RequestContext as read, but after the
    /// agent signs in Rust drives them through X's OAuth 2.0 PKCE
    /// consent screen and emits the minted tokens (`AuthorizeSucceeded`)
    /// for the CLI to persist. Requires the X-App OAuth client creds
    /// below (the CLI reads them from the captured X-App).
    #[arg(long, group = "mode", value_name = "TAG", requires = "x_app_client_id")]
    pub agent_authorize: Option<String>,

    /// X-App OAuth client id — required with `--agent-authorize`. Used for
    /// the PKCE token exchange. Supplied by the CLI from the captured
    /// X-App credentials (the browser no longer reads the DB).
    #[arg(long, requires = "agent_authorize")]
    pub x_app_client_id: Option<String>,

    /// X-App OAuth client secret — required with `--agent-authorize`.
    #[arg(long, requires = "agent_authorize")]
    pub x_app_client_secret: Option<String>,

    /// Launch in Agent **browser** mode, scoped to the given
    /// agent tag. Loads `https://x.com/` under the agent's CEF
    /// profile (shared with `--agent-read` / `--agent-authorize`).
    /// Just opens the browser — no read-scrape, no OAuth flow.
    /// The overlay JS is NOT injected. Operator closes the window
    /// when done.
    #[arg(long, group = "mode", value_name = "TAG")]
    pub agent_browser: Option<String>,

    /// Launch in reply/quote **delivery** mode, scoped to the given agent
    /// tag (one agent per invocation — the CLI spawns one browser per
    /// agent). Shares the agent's `agent-<tag>` CEF profile. Requires
    /// `--agent-deliver-items`; the browser fulfills each item as this agent,
    /// streams one `Output::Delivered` per success, then self-exits.
    #[arg(long, group = "mode", value_name = "TAG", requires = "agent_deliver_items")]
    pub agent_deliver: Option<String>,

    /// The reply/quote payload for `--agent-deliver`: an inline JSON array
    /// of [`psychological_operations_sdk::browser::deliver::DeliverItem`]
    /// (each `{tweet_id, content, kind}` — the agent is `--agent-deliver`,
    /// not a per-item field). Only valid with `--agent-deliver`.
    #[arg(long, value_name = "JSON", requires = "agent_deliver")]
    pub agent_deliver_items: Option<String>,

    /// Launch the Discord bot-creation wizard for the given agent tag.
    /// Lands on the Discord developer portal under the shared `discord`
    /// CEF profile; the overlay guides sign-in + bot creation and scrapes
    /// the bot token, which is stored for the agent.
    #[arg(long, group = "mode", value_name = "TAG")]
    pub discord_login: Option<String>,

    /// Launch the master Twitch **app-setup** wizard. Lands on the Twitch dev
    /// console (`https://dev.twitch.tv/console/apps`) under the flat
    /// `twitch-app` CEF profile; the overlay read-scrapes the app's
    /// `client_id` + `client_secret` and emits `TwitchAppSetupSucceeded` for
    /// the CLI to persist (the X-App analog for Twitch).
    #[arg(long, group = "mode")]
    pub twitch_app: bool,

    /// Launch the per-agent Twitch **authorize** wizard for the given agent
    /// tag. Rust drives Twitch's OAuth code flow (the operator signs into the
    /// agent's Twitch account + consents on Twitch's own pages) and emits the
    /// minted user tokens (`TwitchAuthorizeSucceeded`) for the CLI to persist.
    /// Requires the Twitch app client creds below (the CLI reads them from the
    /// captured master app).
    #[arg(long, group = "mode", value_name = "TAG", requires = "twitch_client_id")]
    pub twitch_authorize: Option<String>,

    /// Twitch app OAuth client id — required with `--twitch-authorize`. Used
    /// for the token exchange + authorize URL. Supplied by the CLI from the
    /// captured master Twitch app (the browser no longer reads the DB).
    #[arg(long, requires = "twitch_authorize")]
    pub twitch_client_id: Option<String>,

    /// Twitch app OAuth client secret — required with `--twitch-authorize`.
    /// Twitch's confidential code flow sends it as a form param (not Basic
    /// auth) during the token exchange.
    #[arg(long, requires = "twitch_authorize")]
    pub twitch_client_secret: Option<String>,

    /// OAuth redirect URI for the Twitch authorize flow. Optional — defaults to
    /// the fixed loopback callback the browser binds
    /// (`http://localhost:17563/psychological-operations/callback`). Must match
    /// the redirect URL registered in the Twitch app EXACTLY. Only meaningful
    /// with `--twitch-authorize`.
    #[arg(long, requires = "twitch_authorize")]
    pub twitch_redirect_uri: Option<String>,

    /// Bytes — SQLite response-cache size budget passed to
    /// `Client::new` when the browser needs to interact with
    /// the X v2 API (today: the OAuth-mint write under
    /// `--agent-authorize`). Default 256 MiB.
    #[arg(long, default_value_t = 256 * 1024 * 1024)]
    pub cache_max_size: u64,

    /// Seconds — response-cache entry TTL passed to
    /// `Client::new`. Currently plumbed but unused; future
    /// work will use it for time-based eviction. Default 3600
    /// (1 hour).
    #[arg(long, default_value_t = 3600)]
    pub cache_ttl: u64,
}

impl Args {
    /// Resolve the CLI mode flags into the SDK's [`Mode`]. The
    /// `unreachable!` is guarded by clap's required+single group
    /// — clap fails parsing if zero or multiple mode flags are
    /// passed.
    pub fn initial_mode(&self) -> Mode {
        if self.x_app {
            Mode::XApp
        } else if let Some(name) = self.agent_read.as_ref() {
            Mode::AgentRead { name: name.clone() }
        } else if let Some(name) = self.agent_authorize.as_ref() {
            Mode::AgentAuthorize { name: name.clone() }
        } else if let Some(name) = self.agent_browser.as_ref() {
            Mode::AgentBrowser { name: name.clone() }
        } else if let Some(name) = self.agent_deliver.as_ref() {
            Mode::AgentDeliver { name: name.clone() }
        } else if let Some(name) = self.discord_login.as_ref() {
            Mode::DiscordLogin { name: name.clone() }
        } else if self.twitch_app {
            Mode::TwitchApp
        } else if let Some(name) = self.twitch_authorize.as_ref() {
            Mode::TwitchAuthorize { name: name.clone() }
        } else {
            unreachable!("clap ArgGroup mode required=true, multiple=false")
        }
    }
}
