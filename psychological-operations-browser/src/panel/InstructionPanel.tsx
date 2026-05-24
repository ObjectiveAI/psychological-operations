import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

type SignedInState = {
  signed_in: boolean;
  info?: {
    session_id?: string;
    handle?: string;
    email?: string;
    user_id?: string;
  };
};

// Rendered into the panel webview's #root. The panel webview lives
// stacked above the content webview, separate JS context, never
// navigates — so we just listen for the `psyops:signed_in` Tauri
// event from the Rust-side cookie watcher and render the panel
// based on state.
//
// Three render branches:
//   signed_in === true  → render nothing (Rust reflows the panel
//                          webview to 0 height so it's visually
//                          absent too)
//   signed_in === false → "Sign in to X."
//   null (unknown)      → "Sign in to X." (default-visible during
//                          the brief window before Rust pushes; if
//                          actually signed-in the push lands within
//                          a frame and the panel hides)
export function InstructionPanel() {
  const [state, setState] = useState<SignedInState | null>(null);

  useEffect(() => {
    // Local webview → IPC is unconditional. No race / capability
    // concerns. Query current state on mount + subscribe to flips.
    invoke<SignedInState | null>("current_signed_in")
      .then(setState)
      .catch(() => {});

    let unlisten: UnlistenFn | undefined;
    listen<SignedInState>("psyops:signed_in", (e) => setState(e.payload))
      .then((u) => {
        unlisten = u;
      })
      .catch(() => {});
    return () => unlisten?.();
  }, []);

  if (state?.signed_in === true) return null;
  return <div style={STYLE}>Sign in to X.</div>;
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
