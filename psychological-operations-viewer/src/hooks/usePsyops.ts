import { useEffect, useState } from "react";
import { invokeCli } from "@objectiveai/sdk/viewer";
import type {
  Psyop,
  PsyopEntry,
  PsyopWithDefinition,
} from "../types/psyop";

// Discriminated union — callers switch on `.status` without
// nullchecking four optional fields.
export type UsePsyopsState =
  | { status: "loading" }
  | { status: "ready"; psyops: PsyopWithDefinition[] }
  | { status: "error"; error: string };

/**
 * On mount, fetches `psyops list` then `psyops get <name>` for each
 * entry and returns the merged result. Sequential, not concurrent
 * — see the comment block inside the effect for why.
 *
 * Re-renders ARE NOT trigger a refetch — the effect has an empty
 * dep array. Returns the same state across the component's
 * lifetime until unmount.
 */
export function usePsyops(): UsePsyopsState {
  const [state, setState] = useState<UsePsyopsState>({ status: "loading" });

  useEffect(() => {
    let cancelled = false;

    void (async () => {
      try {
        const entries = await fetchList();
        if (cancelled) return;

        // SEQUENTIAL — not concurrent. `@objectiveai/sdk/viewer`'s
        // `invokeCli` has no per-invocation demux; firing two in
        // parallel produces interleaved streams the iterators
        // can't separate (each iterator's `onMessage` listener
        // sees every `cli_command` event, regardless of which
        // invocation produced it — see viewer/index.ts:142-146).
        // If concurrency becomes a perf concern, the right fix is
        // either a batch endpoint (`psyops list --details`) or
        // per-invocation id support in objectiveai-sdk.
        const full: PsyopWithDefinition[] = [];
        for (const entry of entries) {
          if (cancelled) return;
          const definition = await fetchPsyop(entry.name);
          full.push({ ...entry, definition });
        }
        if (cancelled) return;
        setState({ status: "ready", psyops: full });
      } catch (e) {
        if (cancelled) return;
        setState({
          status: "error",
          error: e instanceof Error ? e.message : String(e),
        });
      }
    })();

    return () => {
      cancelled = true;
    };
  }, []);

  return state;
}

async function fetchList(): Promise<PsyopEntry[]> {
  return await extractNotificationValue<PsyopEntry[]>(
    invokeCli(["psyops", "list"]),
    "psyops list",
  );
}

async function fetchPsyop(name: string): Promise<Psyop> {
  return await extractNotificationValue<Psyop>(
    invokeCli(["psyops", "get", name]),
    `psyops get ${name}`,
  );
}

/**
 * Drain the cli's JSONL stream and return the `.value.value` of
 * its single `notification` line. psyops cli emits exactly one
 * notification per command (the result payload), wrapped per
 * `emit_notification_from_payload`'s `{value: <payload>}`
 * envelope and then re-wrapped by the host's `Output<Value>`
 * frame — hence the double-`value` access.
 *
 * Throws if the cli emits an `error` line, or if the stream
 * completes without a notification payload.
 */
async function extractNotificationValue<T>(
  iter: AsyncIterable<unknown>,
  label: string,
): Promise<T> {
  let payload: T | undefined;
  let errorMsg: string | undefined;

  for await (const line of iter) {
    if (typeof line !== "object" || line === null) continue;
    const o = line as Record<string, unknown>;

    if (o.type === "error") {
      const msg = typeof o.message === "string" ? o.message : "(no message)";
      errorMsg = `${label}: ${msg}`;
      continue;
    }
    if (o.type !== "notification") continue;
    // Wire shape: {"type":"notification","value":{"value":<T>}}
    const outer = o.value;
    if (typeof outer !== "object" || outer === null) continue;
    const inner = (outer as Record<string, unknown>).value;
    if (inner === undefined) continue;
    payload = inner as T;
  }

  if (errorMsg !== undefined) throw new Error(errorMsg);
  if (payload === undefined) {
    throw new Error(`${label}: no notification payload received`);
  }
  return payload;
}
