//! CSP-immune JS↔Rust IPC via CEF V8 native bindings.
//!
//! x.com's strict CSP blocks fetches to non-listed schemes,
//! which kills the `psyops://invoke/<cmd>` transport we use
//! everywhere else. CSP applies to JavaScript-initiated network
//! operations; V8 functions installed via CEF's
//! [`RenderProcessHandler::on_context_created`] are native code
//! attached to the JS runtime itself and live entirely outside
//! CSP's scope.
//!
//! ## Architecture
//!
//! Renderer process (this module):
//!   - [`OverlayRenderProcessHandler`] runs in every renderer
//!     subprocess. On every V8 context creation it installs a
//!     `window.__psyops_send(envelope_json)` global, backed by
//!     [`OverlayV8Handler`].
//!   - When JS calls `__psyops_send`, the handler reads the
//!     envelope string, packages it as a CEF
//!     [`ProcessMessage`] named `"psyops_invoke"`, and sends
//!     it to the browser process via
//!     [`Frame::send_process_message`].
//!
//! Browser process ([`crate::cef::ContentClient::on_process_message_received`]):
//!   - Receives `"psyops_invoke"`, calls
//!     [`crate::cef_scheme::dispatch_inner`] (the same shared
//!     dispatcher the HTTP scheme handler uses), and ships
//!     the response back as `window.__psyops_recv(corrid,
//!     status, result_json)` via
//!     [`crate::cef::execute_overlay_js`].
//!
//! Renderer-side fulfilment: `__psyops_recv` is registered by
//! the overlay's `ipc.ts`. Pure JS Promise machinery resolves
//! pending invokes on `corrid` match.
//!
//! ## Envelope shape
//!
//! Single string argument to keep the V8 binding minimal:
//!
//! ```json
//! {"corrid": 42, "cmd": "current_mode", "args": "{}"}
//! ```
//!
//! `args` is itself a JSON-string (not a value) so the bridge
//! doesn't need to introspect it; the Rust dispatcher
//! re-parses per-command.

use cef::*;

const FUNCTION_NAME: &str = "__psyops_send";
const MESSAGE_NAME: &str = "psyops_invoke";

wrap_v8_handler! {
    struct OverlayV8Handler {}

    impl V8Handler {
        fn execute(
            &self,
            name: Option<&CefString>,
            _object: Option<&mut V8Value>,
            arguments: Option<&[Option<V8Value>]>,
            _retval: Option<&mut Option<V8Value>>,
            _exception: Option<&mut CefString>,
        ) -> i32 {
            // Defensive: this handler is bound under the
            // canonical name only, but the contract for V8
            // handlers includes the function name in case the
            // same handler is reused across bindings.
            if name.map(|n| n.to_string()) != Some(FUNCTION_NAME.into()) {
                return 0;
            }
            let Some(args) = arguments else { return 0 };
            // Single envelope string argument.
            let Some(Some(envelope)) = args.first() else { return 0 };
            if envelope.is_string() == 0 {
                return 0;
            }
            let envelope_str = CefStringUtf16::from(&envelope.string_value()).to_string();

            // Build the process message + ship to browser.
            let msg_name = CefString::from(MESSAGE_NAME);
            let Some(mut msg) = process_message_create(Some(&msg_name)) else {
                return 0;
            };
            if let Some(arg_list) = msg.argument_list() {
                let envelope_cef = CefString::from(envelope_str.as_str());
                arg_list.set_string(0, Some(&envelope_cef));
            }

            // The frame the call originated from. Reaches into
            // the V8 current-context to find it — works for
            // main frame, iframes, all hosts.
            let Some(ctx) = v8_context_get_current_context() else {
                return 0;
            };
            let Some(frame) = ctx.frame() else { return 0 };
            frame.send_process_message(ProcessId::BROWSER, Some(&mut msg));
            1
        }
    }
}

wrap_render_process_handler! {
    pub struct OverlayRenderProcessHandler {}

    impl RenderProcessHandler {
        fn on_context_created(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            context: Option<&mut V8Context>,
        ) {
            let Some(ctx) = context else { return };
            let Some(global) = ctx.global() else { return };
            let mut handler = OverlayV8Handler::new();
            let name_cef = CefString::from(FUNCTION_NAME);
            let Some(mut function) =
                v8_value_create_function(Some(&name_cef), Some(&mut handler))
            else {
                return;
            };
            // V8_PROPERTY_ATTRIBUTE_NONE (0) via the zeroed
            // Default impl — writable, enumerable, configurable.
            let attr = V8Propertyattribute::default();
            global.set_value_bykey(Some(&name_cef), Some(&mut function), attr);
        }
    }
}

/// Parse a `psyops_invoke` envelope (`{corrid, cmd, args}`) out
/// of a single-string-argument [`ProcessMessage`]. Called by
/// [`crate::cef::ContentClient::on_process_message_received`].
pub fn parse_envelope(message: &mut ProcessMessage) -> Option<Envelope> {
    let arg_list = message.argument_list()?;
    if arg_list.size() < 1 {
        return None;
    }
    let envelope_str = arg_list.string(0);
    let envelope_str = CefStringUtf16::from(&envelope_str).to_string();
    serde_json::from_str::<Envelope>(&envelope_str).ok()
}

/// Decoded `psyops_invoke` envelope.
#[derive(Debug, serde::Deserialize)]
pub struct Envelope {
    pub corrid: i64,
    pub cmd: String,
    /// JSON-encoded args object (or `"{}"` for no-arg commands).
    pub args: String,
}

/// True iff `message`'s name is the `"psyops_invoke"` envelope
/// we care about. Filter the process-message stream before
/// parsing; CEF delivers other internal messages we have no
/// business interpreting.
pub fn is_invoke(message: &ProcessMessage) -> bool {
    let name = message.name();
    CefStringUtf16::from(&name).to_string() == MESSAGE_NAME
}
