//! "Just open the browser as this agent" flow for `agents browser <tag>`.
//!
//! The browser loads `https://x.com/` under the persona's CEF
//! profile and waits for the operator to close the window. No
//! pre-flight, no terminator, no `Request::Shutdown` — the browser
//! exits when the operator clicks the X. The overlay JS is gated
//! out in the tauri side's `cef.rs` for these modes, so the X.com
//! page runs untouched.
//!
//! The only mode-specific behavior on the panel side is the
//! `SignInToX` nag (when the persona isn't signed in to X.com yet);
//! see `state.rs::derive` for the browser-mode arm.

use psychological_operations_sdk::browser::auth_json::PersonaKind;
use psychological_operations_sdk::cli::Output;

use crate::browser::{browser_binary, launch};
use crate::error::Error;

pub async fn run(kind: PersonaKind, name: &str, ctx: &crate::context::Context) -> bool {
    crate::output::emit_result(run_inner(kind, name, ctx).await)
}

async fn run_inner(
    kind: PersonaKind,
    name: &str,
    ctx: &crate::context::Context,
) -> Result<Output, Error> {
    // Opening the persona browser is inherently a real-browser action;
    // there is nothing to mock. Refuse outright in mock mode.
    if ctx.config.mock {
        return Err(Error::Other(
            "browser is not supported in mock mode (PSYCHOLOGICAL_OPERATIONS_MOCK)".into(),
        ));
    }

    let state_dir = ctx.config.state_dir();
    let launch_mode = match kind {
        PersonaKind::Agent => launch::Mode::AgentBrowser {
            name: name.to_string(),
        },
    };
    let event_kind = match kind {
        PersonaKind::Agent => "agent_browser",
    };

    // Inherit stdin/stdout — no terminator stream, no shutdown
    // request, the operator closes the window when done.
    let mut child = launch::spawn(
        &browser_binary(&ctx.config),
        &state_dir,
        launch_mode,
        /* pipe_stdin  = */ false,
        /* pipe_stdout = */ false,
    )?;

    crate::output::OutputResult::from(crate::events::Event::BrowserSpawned {
        kind: event_kind.into(),
        name: Some(name.to_string()),
        pid: child.id(),
    })
    .emit();

    let status = child
        .wait()
        .map_err(|e| Error::Other(format!("waiting for browser ({name}) failed: {e}")))?;

    crate::output::OutputResult::from(crate::events::Event::BrowserExit {
        kind: event_kind.into(),
        name: Some(name.to_string()),
        status: status.code(),
    })
    .emit();

    Ok(Output::Ok)
}
