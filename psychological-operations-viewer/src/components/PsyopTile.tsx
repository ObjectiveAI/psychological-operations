import cn from "classnames";
import { invokeCli } from "@objectiveai/sdk/viewer";
import type { PsyopWithDefinition, SortBy } from "../types/psyop";

interface PsyopTileProps {
  psyop: PsyopWithDefinition;
}

export function PsyopTile({ psyop }: PsyopTileProps) {
  const onRun = () => {
    // Fire-and-forget. We still need to drain the iterator so the
    // SDK's `message` event listener self-removes (otherwise it
    // leaks for the lifetime of the iframe).
    void (async () => {
      try {
        const iter = invokeCli(["psyops", "run", "--name", psyop.name]);
        for await (const _line of iter) {
          // No-op for now; progress / completion UI is a follow-up.
        }
      } catch (e) {
        // eslint-disable-next-line no-console
        console.error(`psyops run ${psyop.name} failed:`, e);
      }
    })();
  };

  return (
    <article
      className={cn(
        "w-72",
        "shrink-0",
        "flex",
        "flex-col",
        "gap-3",
        "p-4",
        "rounded-lg",
        "border",
        "border-black/10",
        "dark:border-white/10",
      )}
    >
      <header
        className={cn("flex", "items-center", "justify-between", "gap-2")}
      >
        <h2 className={cn("font-medium", "truncate")} title={psyop.name}>
          {psyop.name}
        </h2>
        <span
          className={cn(
            "text-xs",
            "px-2",
            "py-0.5",
            "rounded-full",
            psyop.enabled && "bg-green-500/15",
            psyop.enabled && "text-green-700",
            !psyop.enabled && "bg-gray-500/15",
            !psyop.enabled && "text-gray-600",
          )}
        >
          {psyop.enabled ? "enabled" : "disabled"}
        </span>
      </header>

      <dl className={cn("text-sm", "grid", "grid-cols-2", "gap-y-1")}>
        <dt className={cn("opacity-60")}>stages</dt>
        <dd>{psyop.definition.stages.length}</dd>
        <dt className={cn("opacity-60")}>sort</dt>
        <dd className={cn("truncate")}>{formatSort(psyop.definition.sort)}</dd>
        <dt className={cn("opacity-60")}>commit</dt>
        <dd className={cn("font-mono", "text-xs")}>
          {psyop.commit_sha.slice(0, 7)}
        </dd>
      </dl>

      <button
        type="button"
        onClick={onRun}
        className={cn(
          "mt-auto",
          "px-3",
          "py-1.5",
          "rounded",
          "bg-blue-600",
          "text-white",
          "text-sm",
          "font-medium",
          "hover:bg-blue-700",
          "active:bg-blue-800",
          "transition-colors",
        )}
      >
        Run
      </button>
    </article>
  );
}

function formatSort(s: SortBy): string {
  if (typeof s === "string") return s;
  return `custom: ${s.custom}`;
}
