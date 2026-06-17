import { useCallback, useState } from "react";
import cn from "classnames";
import { runCli } from "./cli";
import { usePsyops } from "./hooks/usePsyops";
import { LoadingEllipsis } from "./components/LoadingEllipsis";
import { PsyopTile } from "./components/PsyopTile";

export function App() {
  const state = usePsyops();

  // One run at a time, globally. The viewer→host cli transport has no
  // per-invocation demux — concurrent runs would interleave their output
  // streams (no per-call sub_type on cli_command events) and each could
  // terminate on another's end marker.
  const [runningPsyop, setRunningPsyop] = useState<string | null>(null);
  const [outputs, setOutputs] = useState<Record<string, string[]>>({});

  const handleRun = useCallback(
    (name: string) => {
      if (runningPsyop !== null) return;
      setRunningPsyop(name);
      setOutputs((prev) => ({ ...prev, [name]: [] }));

      void (async () => {
        try {
          const iter = runCli(["psyops", "run", "--name", name]);
          for await (const line of iter) {
            // ONE setState per yielded line so React renders
            // between awaits — that's what surfaces lines "as
            // they come in" rather than after the run finishes.
            const text =
              typeof line === "string" ? line : JSON.stringify(line);
            setOutputs((prev) => ({
              ...prev,
              [name]: [...(prev[name] ?? []), text],
            }));
          }
        } catch (e) {
          const msg = e instanceof Error ? e.message : String(e);
          setOutputs((prev) => ({
            ...prev,
            [name]: [...(prev[name] ?? []), `(error) ${msg}`],
          }));
        } finally {
          setRunningPsyop(null);
        }
      })();
    },
    [runningPsyop],
  );

  return (
    <main className={cn("h-screen", "flex", "flex-col", "p-6", "gap-4")}>
      <header className={cn("flex", "items-baseline", "gap-3")}>
        <h1 className={cn("text-xl", "font-semibold")}>
          psychological-operations
        </h1>
        {state.status === "ready" && (
          <span className={cn("text-sm", "opacity-60")}>
            {state.psyops.length} psyop{state.psyops.length === 1 ? "" : "s"}
          </span>
        )}
      </header>

      {state.status === "loading" && (
        <div
          className={cn(
            "flex-1",
            "flex",
            "items-center",
            "justify-center",
          )}
        >
          <LoadingEllipsis />
        </div>
      )}

      {state.status === "error" && (
        <div
          className={cn(
            "flex-1",
            "flex",
            "items-center",
            "justify-center",
            "text-red-600",
          )}
        >
          {state.error}
        </div>
      )}

      {state.status === "ready" && state.psyops.length === 0 && (
        <div
          className={cn(
            "flex-1",
            "flex",
            "items-center",
            "justify-center",
            "opacity-60",
          )}
        >
          No psyops yet.
        </div>
      )}

      {state.status === "ready" && state.psyops.length > 0 && (
        <div
          className={cn(
            "flex",
            "flex-row",
            "items-start",
            "gap-4",
            "overflow-x-auto",
            "pb-2",
          )}
        >
          {state.psyops.map((p) => (
            <PsyopTile
              key={`${p.name}@${p.commit_sha}`}
              psyop={p}
              isRunning={runningPsyop === p.name}
              isAnyRunning={runningPsyop !== null}
              output={outputs[p.name]}
              onRun={() => handleRun(p.name)}
            />
          ))}
        </div>
      )}
    </main>
  );
}
