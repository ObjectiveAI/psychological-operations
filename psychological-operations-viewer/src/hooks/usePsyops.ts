import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@objectiveai/sdk/viewer";
import { runCli } from "../cli";
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
 * entry and returns the merged result.
 *
 * Also subscribes to three host-pushed events the cli fires after
 * CRUD ops (see `objectiveai.json::viewer_routes`):
 *
 *  - `psyop_added` / `psyop_edited`: payload is a `PsyopWithDefinition`.
 *    If we're already `ready`, mutate state.psyops in place (replace
 *    by name, or append). If we're still `loading` or in `error`,
 *    discard the in-flight fetch via a generation counter and start
 *    a fresh full reload — the notification implies the on-disk state
 *    just changed, and racing partial data against the newer truth
 *    is worse than re-fetching cheaply.
 *
 *  - `psyop_deleted`: payload is `{name}`. In `ready`, drop by name.
 *    In `loading`/`error`, restart the fetch.
 *
 * Initial fetch runs once on mount; subsequent fetches only happen
 * via the loading-state notification handler. The hook doesn't
 * expose a `refresh()` callback — if the cli does CRUD, it'll
 * notify us; if you want manual refresh, add it later.
 */
export function usePsyops(): UsePsyopsState {
  const [state, setState] = useState<UsePsyopsState>({ status: "loading" });

  // Mirror state to a ref so async listeners read the latest status
  // without recapturing closures (listen() handlers are registered
  // once per mount and outlive any individual render's `state`).
  const stateRef = useRef<UsePsyopsState>(state);

  // Generation guard: each call to `startFetch()` bumps this. The
  // in-flight async fetch checks the gen before every `setState` —
  // if a notification triggered a newer fetch while we were
  // mid-flight, this older fetch's setState calls become no-ops.
  const genRef = useRef(0);

  const setStateWithRef = useCallback((s: UsePsyopsState) => {
    stateRef.current = s;
    setState(s);
  }, []);

  const startFetch = useCallback(() => {
    const gen = ++genRef.current;
    setStateWithRef({ status: "loading" });

    void (async () => {
      try {
        const entries = await fetchList();
        if (genRef.current !== gen) return;

        // SEQUENTIAL — not concurrent. The viewer→host cli transport has
        // no per-invocation demux; firing two in parallel produces
        // interleaved streams the iterators can't separate.
        const full: PsyopWithDefinition[] = [];
        for (const entry of entries) {
          if (genRef.current !== gen) return;
          const definition = await fetchPsyop(entry.name);
          full.push({ ...entry, definition });
        }
        if (genRef.current !== gen) return;
        setStateWithRef({ status: "ready", psyops: full });
      } catch (e) {
        if (genRef.current !== gen) return;
        setStateWithRef({
          status: "error",
          error: e instanceof Error ? e.message : String(e),
        });
      }
    })();
  }, [setStateWithRef]);

  // Initial fetch on mount.
  useEffect(() => {
    startFetch();
  }, [startFetch]);

  // Subscribe to push notifications from cli CRUD ops.
  useEffect(() => {
    const onUpsert = (value: unknown) => {
      const current = stateRef.current;
      if (current.status !== "ready") {
        // Pre-load or error path: discard in-flight, restart fresh.
        startFetch();
        return;
      }
      const psyop = value as PsyopWithDefinition;
      if (!psyop || typeof psyop.name !== "string") return;
      const idx = current.psyops.findIndex((p) => p.name === psyop.name);
      const next =
        idx >= 0
          ? current.psyops.map((p, i) => (i === idx ? psyop : p))
          : [...current.psyops, psyop];
      setStateWithRef({ status: "ready", psyops: next });
    };

    const onDeleted = (value: unknown) => {
      const current = stateRef.current;
      if (current.status !== "ready") {
        startFetch();
        return;
      }
      const obj = value as { name?: unknown } | null;
      if (!obj || typeof obj.name !== "string") return;
      const name = obj.name;
      const next = current.psyops.filter((p) => p.name !== name);
      // Missing-name delete is a no-op (lenient — could be a duplicate
      // event after a reload, or a race where the viewer already
      // dropped the entry).
      if (next.length === current.psyops.length) return;
      setStateWithRef({ status: "ready", psyops: next });
    };

    const offAdded = listen("psyop_added", onUpsert);
    const offEdited = listen("psyop_edited", onUpsert);
    const offDeleted = listen("psyop_deleted", onDeleted);

    return () => {
      offAdded();
      offEdited();
      offDeleted();
    };
  }, [startFetch, setStateWithRef]);

  return state;
}

async function fetchList(): Promise<PsyopEntry[]> {
  return await extractNotificationValue<PsyopEntry[]>(
    runCli(["psyops", "list"]),
    "psyops list",
  );
}

async function fetchPsyop(name: string): Promise<Psyop> {
  return await extractNotificationValue<Psyop>(
    runCli(["psyops", "get", name]),
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
