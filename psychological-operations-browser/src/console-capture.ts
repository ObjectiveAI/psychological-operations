// In-page capture of `console.*` calls and uncaught exceptions.
//
// Installed at the very top of `main.tsx` so we catch everything
// from the moment the bundle runs (which, via
// `initialization_script`, is before any page script). Entries
// accumulate in a bounded ring buffer; the host drains them with
// `Request::Console`.
//
// Mirrors what DevTools' Console panel would show, with the
// minimum metadata needed to correlate entries across navigations
// (timestamp + url).

type Level = "log" | "warn" | "error" | "info" | "debug" | "exception";

interface Entry {
  level: Level;
  message: string;
  timestamp: number;
  url: string;
  stack?: string;
}

const BUFFER_LIMIT = 10_000;
let buffer: Entry[] = [];

const push = (entry: Entry) => {
  buffer.push(entry);
  if (buffer.length > BUFFER_LIMIT) {
    buffer.splice(0, buffer.length - BUFFER_LIMIT);
  }
};

const stringify = (v: unknown): string => {
  if (typeof v === "string") return v;
  if (v instanceof Error) return v.stack ?? v.message;
  try {
    return JSON.stringify(v);
  } catch {
    return String(v);
  }
};

const join = (args: unknown[]) => args.map(stringify).join(" ");

let installed = false;

export function installConsoleCapture(): void {
  if (installed) return;
  installed = true;

  // Each navigation produces a fresh page (and a fresh bundle
  // execution via `initialization_script`), so `console` here is
  // the new page's console — patching it once per bundle execution
  // is correct.
  for (const level of ["log", "warn", "error", "info", "debug"] as const) {
    const original = console[level].bind(console);
    console[level] = (...args: unknown[]) => {
      try {
        push({
          level,
          message: join(args),
          timestamp: Date.now(),
          url: location.href,
        });
      } catch {
        // Capture must never break the original call.
      }
      original(...args);
    };
  }

  window.addEventListener("error", (ev) => {
    try {
      push({
        level: "exception",
        message: ev.message || "uncaught error",
        timestamp: Date.now(),
        url: location.href,
        stack: ev.error?.stack,
      });
    } catch {}
  });

  window.addEventListener("unhandledrejection", (ev) => {
    try {
      const reason = ev.reason;
      const isErr = reason instanceof Error;
      push({
        level: "exception",
        message: isErr ? reason.message : stringify(reason),
        timestamp: Date.now(),
        url: location.href,
        stack: isErr ? reason.stack : undefined,
      });
    } catch {}
  });
}

/**
 * Drain (and clear) the buffer. Each call returns only entries
 * pushed since the previous drain — matches `Request::Console`'s
 * drain semantics.
 */
export function drainConsole(): Entry[] {
  const out = buffer;
  buffer = [];
  return out;
}
