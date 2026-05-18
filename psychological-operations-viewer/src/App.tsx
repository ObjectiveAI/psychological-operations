import cn from "classnames";
import { usePsyops } from "./hooks/usePsyops";
import { LoadingEllipsis } from "./components/LoadingEllipsis";
import { PsyopTile } from "./components/PsyopTile";

export function App() {
  const state = usePsyops();

  return (
    <main
      className={cn("h-screen", "flex", "flex-col", "p-6", "gap-4")}
    >
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
            "gap-4",
            "overflow-x-auto",
            "pb-2",
          )}
        >
          {state.psyops.map((p) => (
            <PsyopTile key={`${p.name}@${p.commit_sha}`} psyop={p} />
          ))}
        </div>
      )}
    </main>
  );
}
