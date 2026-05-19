//! Fire-and-forget viewer push notifications fired from psyops CRUD
//! ops (publish / enable / disable / delete). Each shells out to
//! `objectiveai viewer send /plugin/psychological-operations/<route> <body>`
//! which POSTs to the host's axum server. The host emits an
//! `Event::Inbound { destination, sub_type, value }` that the
//! viewer's `listen("<sub_type>", handler)` consumes — see the
//! `viewer_routes` declarations in `objectiveai.json`.
//!
//! Failures are dropped silently: a terminal-only cli invocation
//! commonly has no running viewer, and that's not an error worth
//! surfacing to the user.

use serde_json::Value;

/// Map a `sub_type` to the route path the manifest declares.
/// Keeps wire shape in one place so a typo in `objectiveai.json`
/// produces a compile / lint warning here too (every match arm
/// pairs).
fn route_for(sub_type: &str) -> Option<&'static str> {
    match sub_type {
        "psyop_added"   => Some("/plugin/psychological-operations/psyops/added"),
        "psyop_edited"  => Some("/plugin/psychological-operations/psyops/edited"),
        "psyop_deleted" => Some("/plugin/psychological-operations/psyops/deleted"),
        _ => None,
    }
}

/// Fire one notification at the host's running viewer. No-op if
/// `sub_type` is unknown (we'd rather drop than panic — the cli
/// surface that called us is wider than the route table).
pub fn notify(sub_type: &str, body: &Value, cfg: &crate::run::Config) {
    let Some(path) = route_for(sub_type) else { return; };
    let body_str = body.to_string();
    // Sync command — the cli's CRUD handlers aren't async at the
    // moment, and a once-per-CRUD ~50 ms shell-out is fine for
    // operations that already involve a git commit.
    let _ = std::process::Command::new(crate::score::objectiveai_binary(cfg))
        .args(["viewer", "send", path, &body_str])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}
