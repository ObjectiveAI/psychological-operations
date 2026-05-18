//! PluginOutput JSONL emission helpers — one place that knows the
//! wire-format quirks of `objectiveai-sdk`'s `PluginOutput` enum.
//!
//! All helpers write **one** JSON line to stdout. The objectiveai host
//! reads our stdout line-by-line, re-wraps each parseable
//! `PluginOutput` in its own `Output<T>` frame, and forwards. So the
//! shape an end user sees in a snapshot is the **host's** re-emit, not
//! ours.
//!
//! Wire-shape cheat sheet (host re-emit form, after our `println!`):
//! - `emit_notification(<obj>)`            → `{"type":"notification","value":<obj>}`
//! - `emit_event("foo")`                   → `{"type":"notification","value":{"event":"foo"}}`
//! - `emit_notification_from_payload(s)`   → `{"type":"notification","value":{"value":<parsed-or-string>}}` (double `value` is intentional — see fn doc)

use objectiveai_sdk::cli::output::{Error as PluginError, Level};
use objectiveai_sdk::cli::plugins::PluginOutput;
use serde_json::Value;

/// Emit one `PluginOutput::Notification` line whose host-re-emitted
/// `value` field equals `value`.
///
/// **`value` MUST be a JSON Object.** `PluginOutput` uses
/// `#[serde(tag = "type")]` internal tagging — serde injects the
/// discriminator into the inner object. Non-objects (strings, arrays,
/// numbers) blow up serialization. For non-object payloads use
/// [`emit_notification_from_payload`], which wraps under `{"value": …}`
/// before this is called.
pub fn emit_notification(value: Value) {
    let out = PluginOutput::Notification(value);
    let line = serde_json::to_string(&out)
        .expect("PluginOutput serializes");
    println!("{line}");
}

/// Emit a typed `Event` through the right `PluginOutput` variant.
///
/// Variants tagged as failures by [`Event::error_level`] go through
/// `emit_error(level, fatal=false, …)`; everything else goes through
/// `emit_notification`. Either way the serialized value carries the
/// `event` discriminator + per-variant fields, so consumers see a
/// uniform shape regardless of the wire variant chosen.
pub fn emit(event: crate::events::Event) {
    let value = serde_json::to_value(&event).expect("Event serializes");
    match event.error_level() {
        Some(level) => emit_error(level, /* fatal */ false, value),
        None        => emit_notification(value),
    }
}

/// Emit a `PluginOutput::Error` line. Caller decides whether to also
/// `std::process::exit(1)` afterward.
///
/// `message` is a `serde_json::Value` since 2.0.5 — pass a string via
/// [`Value::String`] / `json!("text")` if that's all you have.
pub fn emit_error(level: Level, fatal: bool, message: Value) {
    let out = PluginOutput::Error(PluginError { level, fatal, message });
    let line = serde_json::to_string(&out)
        .expect("PluginOutput serializes");
    println!("{line}");
}

/// Wrap a command's final-result payload (the `Display` form of
/// `Output::Api(...)` / `Output::ConfigGet(...)` etc.) as a
/// `PluginOutput::Notification` and emit it.
///
/// The host's re-emit produces
/// `{"type":"notification","value":{"value":<parsed>}}` — the outer
/// `value` is the host's own wrapper, the inner `value` is ours. The
/// inner wrap is required because the parsed payload may be a JSON
/// array / string / number (not an Object), which `PluginOutput`'s
/// internal tagging can't carry directly.
pub fn emit_notification_from_payload(payload: &str) {
    let value: Value = serde_json::from_str(payload)
        .unwrap_or_else(|_| Value::String(payload.to_string()));
    emit_notification(serde_json::json!({ "value": value }));
}
