import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

// Wire shape mirrors `PanelState` in the Rust SDK (see
// `psychological-operations-browser-sdk/src/panel.rs`). All
// derivation lives Rust-side — this component only renders.
type PanelState =
  | { type: "hidden" }
  | { type: "show"; condition: string; message: string };

// Rendered into the panel webview's #root. The panel webview lives
// stacked above the content webview, separate JS context, never
// navigates — so we just listen for the `psyops:panel` Tauri event
// from the Rust-side state module and render whatever message the
// derivation hands us. No conditions, no branches per message —
// adding a new instruction is a Rust-only change.
export function InstructionPanel() {
  const [state, setState] = useState<PanelState | null>(null);

  useEffect(() => {
    // Local webview → IPC is unconditional. No race / capability
    // concerns. Query current state on mount + subscribe to flips.
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

  if (state?.type !== "show") return null;
  // `loading` renders as three flashing dots — see the
  // `psyops-dot` rules in panel.html. Message field is empty by
  // convention for this condition.
  if (state.condition === "loading") {
    return (
      <div style={STYLE}>
        <span className="psyops-dot">.</span>
        <span className="psyops-dot">.</span>
        <span className="psyops-dot">.</span>
      </div>
    );
  }
  return <div style={STYLE}>{state.message}</div>;
}

const STYLE: React.CSSProperties = {
  position: "absolute",
  inset: 0,
  display: "flex",
  alignItems: "center",
  padding: "0 16px",
  color: "#fff",
  font: "14px/1.4 system-ui, -apple-system, Segoe UI, sans-serif",
  letterSpacing: "0.02em",
  background: "rgba(15, 17, 21, 0.92)",
};
