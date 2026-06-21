import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

// Wire shape mirrors `PanelState` in the Rust SDK (see
// `psychological-operations-sdk/src/browser/panel.rs`). All
// derivation lives Rust-side — this component only renders.
type DiscordField = { value?: string; saving: boolean };
type PanelState =
  | { type: "hidden" }
  | { type: "show"; condition: string; message: string }
  | {
      type: "discord_auth";
      application_id: DiscordField;
      public_key: DiscordField;
      bot_token: DiscordField;
    };

// Rendered into the panel webview's #root. The panel webview lives
// stacked above the content webview, separate JS context, never
// navigates — so we just listen for the `psyops:panel` Tauri event
// from the Rust-side state module and render whatever it hands us.
export function InstructionPanel() {
  const [state, setState] = useState<PanelState | null>(null);

  useEffect(() => {
    invoke<PanelState | null>("current_panel")
      .then(setState)
      .catch(() => {});

    let unlisten: UnlistenFn | undefined;
    listen<PanelState>("psyops:panel", (e) => setState(e.payload))
      .then((u) => {
        unlisten = u;
      })
      .catch(() => {});
    return () => unlisten?.();
  }, []);

  if (!state || state.type === "hidden") return null;

  // DiscordLogin: a persistent read-only auth form (the header for the whole
  // Discord session).
  if (state.type === "discord_auth") {
    return (
      <div style={STYLE} className="psyops-form">
        <Field label="Application ID" field={state.application_id} />
        <Field label="Public Key" field={state.public_key} />
        <Field label="Bot Token" field={state.bot_token} />
      </div>
    );
  }

  // `loading` renders as three flashing dots — see the `psyops-dot`
  // rules in panel.html. Message field is empty by convention.
  if (state.condition === "loading") {
    return (
      <div style={STYLE}>
        <Dots />
      </div>
    );
  }
  return (
    <div style={STYLE} className="psyops-pulse">
      {state.message}
    </div>
  );
}

function Dots() {
  return (
    <>
      <span className="psyops-dot">.</span>
      <span className="psyops-dot">.</span>
      <span className="psyops-dot">.</span>
    </>
  );
}

function Field({ label, field }: { label: string; field: DiscordField }) {
  return (
    <div className="psyops-field">
      <span className="psyops-field-label">{label}</span>
      {field.saving ? (
        <span className="psyops-field-dots">
          <Dots />
        </span>
      ) : field.value ? (
        <span className="psyops-field-value" title={field.value}>
          {field.value}
        </span>
      ) : (
        <span className="psyops-field-empty">—</span>
      )}
    </div>
  );
}

const STYLE: React.CSSProperties = {
  position: "absolute",
  inset: 0,
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
  padding: "0 16px",
  color: "#fff",
  font: "14px/1.4 system-ui, -apple-system, Segoe UI, sans-serif",
  letterSpacing: "0.02em",
  background: "rgba(15, 17, 21, 0.92)",
};
