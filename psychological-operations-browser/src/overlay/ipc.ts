// IPC shim for the CEF content overlay.
//
// CEF doesn't host Tauri's IPC machinery (no `window.__TAURI_INTERNALS__`),
// so the overlay can't call Tauri commands directly. Both directions
// of the IPC are bridged through CEF-native primitives instead:
//
//   - JS → Rust: `fetch("psyops://invoke/<cmd>", { method: "POST", body: JSON })`.
//     Rust's psyops:// custom scheme handler (src-tauri/src/cef_scheme.rs)
//     dispatches to inner command functions and returns a JSON body.
//   - Rust → JS: Rust calls
//     `frame.execute_javascript("window.__psyops.push(<json>)")`.
//     We register that handler via [`registerPushHandler`] at mount.
//
// API mirrors what Tauri offered: `invoke(cmd, args)` returns a Promise
// resolving to the JSON-decoded response; `registerPushHandler(fn)` is
// the rough equivalent of `listen("psyops:request", fn)` (single
// in-flight request at a time, no envelope, single global handler).

export async function invoke<T = unknown>(
  cmd: string,
  args: unknown = {},
): Promise<T> {
  const response = await fetch(`psyops://invoke/${cmd}`, {
    method: "POST",
    body: JSON.stringify(args),
    headers: { "content-type": "application/json" },
  });
  if (!response.ok) {
    const text = await response.text().catch(() => "");
    throw new Error(`psyops://invoke/${cmd} ${response.status}: ${text}`);
  }
  const ct = response.headers.get("content-type") ?? "";
  if (ct.includes("json")) {
    return (await response.json()) as T;
  }
  return (await response.text()) as unknown as T;
}

export type RequestHandler = (req: unknown) => void | Promise<void>;

/// Register the handler Rust pushes requests to via
/// `Frame::execute_javascript("window.__psyops.push(<json>)")`.
/// Call this at overlay mount BEFORE invoking `frontend_ready` so
/// the Rust stdin reader isn't unblocked until the handler exists.
export function registerPushHandler(handler: RequestHandler): void {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (window as any).__psyops = {
    push: (req: unknown) => {
      // Best-effort isolate handler errors so a thrown exception
      // here doesn't corrupt CEF's V8 callback frame.
      try {
        const result = handler(req);
        if (result && typeof (result as Promise<void>).catch === "function") {
          (result as Promise<void>).catch(() => {});
        }
      } catch {
        // swallow
      }
    },
  };
}
