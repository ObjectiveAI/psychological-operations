// IPC shim for the CEF content overlay — V8 native bridge.
//
// Transports:
//
//   JS → Rust: a single envelope string is handed to
//   `window.__psyops_send`, which is a CEF V8 native function
//   installed by `RenderProcessHandler::on_context_created`
//   (src-tauri/src/cef_v8.rs). The native handler decodes the
//   envelope and ships it to the browser process as a CEF
//   `ProcessMessage`. Native bindings are not subject to CSP
//   (CSP governs page-script network access, not native
//   runtime extensions), which lets us reach Rust from x.com
//   pages whose CSP would otherwise block a fetch to the
//   `psyops://` scheme.
//
//   Rust → JS (responses): Rust's
//   `ContentClient::on_process_message_received` dispatches via
//   the shared `cef_scheme::dispatch_inner` and pushes the
//   result back as
//   `window.__psyops_recv(corrid, status, result_json)` via
//   `cef::execute_overlay_js`. We register `__psyops_recv`
//   below at module load; it resolves the matching pending
//   Promise.
//
//   Rust → JS (server-initiated pushes, separate channel):
//   Rust calls `window.__psyops.push(<request-json>)` for stdin
//   requests that need overlay handling. We register the
//   handler via [`registerPushHandler`] at overlay mount.
//
// API mirrors what Tauri offered: `invoke(cmd, args)` returns
// a Promise resolving to the JSON-decoded response;
// `registerPushHandler(fn)` is the rough equivalent of
// `listen("psyops:request", fn)`.

type Pending = {
  resolve: (value: unknown) => void;
  reject: (error: unknown) => void;
};

const pending = new Map<number, Pending>();
let nextCorrId = 1;

// Receive responses from Rust. Rust runs this from
// `execute_overlay_js` after the browser-side dispatch
// completes. `result` is a JSON-encoded string (or `"null"` /
// `"undefined"` for no-value commands); we parse it before
// resolving so callers get the structured value.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
(window as any).__psyops_recv = (
  corrid: number,
  status: "ok" | "err",
  result: string,
): void => {
  const p = pending.get(corrid);
  if (!p) return;
  pending.delete(corrid);
  if (status === "ok") {
    try {
      p.resolve(result.length === 0 ? null : JSON.parse(result));
    } catch (e) {
      p.reject(new Error(`psyops: invalid JSON in response: ${(e as Error).message}`));
    }
  } else {
    p.reject(new Error(result));
  }
};

export async function invoke<T = unknown>(
  cmd: string,
  args: unknown = {},
): Promise<T> {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const send = (window as any).__psyops_send as
    | ((envelope: string) => void)
    | undefined;
  if (typeof send !== "function") {
    throw new Error(
      "psyops: __psyops_send is not bound — V8 native bridge not initialized",
    );
  }
  const corrid = nextCorrId++;
  return new Promise<T>((resolve, reject) => {
    pending.set(corrid, {
      resolve: resolve as (v: unknown) => void,
      reject,
    });
    const envelope = JSON.stringify({
      corrid,
      cmd,
      args: JSON.stringify(args),
    });
    try {
      send(envelope);
    } catch (e) {
      pending.delete(corrid);
      reject(e);
    }
  });
}

export type RequestHandler = (req: unknown) => void | Promise<void>;

/// Register the handler Rust pushes requests to via
/// `Frame::execute_javascript("window.__psyops.push(<json>)")`.
/// Call this at overlay mount BEFORE invoking `frontend_ready`
/// so the Rust stdin reader isn't unblocked until the handler
/// exists.
export function registerPushHandler(handler: RequestHandler): void {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (window as any).__psyops = {
    push: (req: unknown) => {
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
