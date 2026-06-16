//! Fire-and-forget viewer push notifications fired from psyops CRUD
//! ops (publish / enable / disable / delete). Each routes through
//! the in-process `viewer send` command via the SDK's
//! `PluginExecutor`, which POSTs to the host's axum server. The
//! host emits an `Event::Inbound { destination, sub_type, value }`
//! that the viewer's `listen("<sub_type>", handler)` consumes —
//! see the `viewer_routes` declarations in `objectiveai.json`.
//!
//! Failures are dropped silently: a terminal-only cli invocation
//! commonly has no running viewer, and that's not an error worth
//! surfacing to the user.

use objectiveai_sdk::cli::command::viewer::send as viewer_send;
use serde_json::Value;

/// Map a `sub_type` to the route path the manifest declares.
fn route_for(sub_type: &str) -> Option<&'static str> {
    match sub_type {
        "psyop_added" => Some("/plugin/psychological-operations/psyops/added"),
        "psyop_edited" => Some("/plugin/psychological-operations/psyops/edited"),
        _ => None,
    }
}

/// Fire one notification at the host's running viewer. No-op if
/// `sub_type` is unknown.
pub async fn notify(sub_type: &str, body: &Value, ctx: &crate::context::Context) {
    let Some(path) = route_for(sub_type) else {
        return;
    };
    let req = viewer_send::Request {
        path_type: viewer_send::Path::ViewerSend,
        path: path.to_string(),
        body: body.clone(),
        base: Default::default(),
    };
    let _ = viewer_send::execute(&*ctx.executor, req, None).await;
}
