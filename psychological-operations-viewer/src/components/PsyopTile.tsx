import cn from "classnames";
import type { PsyopWithDefinition, SortBy } from "../types/psyop";

interface PsyopTileProps {
  psyop: PsyopWithDefinition;
  /** This tile's psyop is the one currently running. */
  isRunning: boolean;
  /** Some psyop (this or another) is currently running. Gates the button. */
  isAnyRunning: boolean;
  /** JSONL lines emitted by the most recent run of this psyop. */
  output: string[] | undefined;
  onRun: () => void;
}

export function PsyopTile({
  psyop,
  isRunning,
  isAnyRunning,
  output,
  onRun,
}: PsyopTileProps) {
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
        <dd>{psyop.definition.stages?.length ?? 0}</dd>
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
        disabled={isAnyRunning}
        className={cn(
          "mt-auto",
          "px-3",
          "py-1.5",
          "rounded",
          "text-white",
          "text-sm",
          "font-medium",
          "transition-colors",
          !isAnyRunning && "bg-blue-600",
          !isAnyRunning && "hover:bg-blue-700",
          !isAnyRunning && "active:bg-blue-800",
          // Disabled-but-this-is-the-running-one: keep blue so the
          // user sees which psyop owns the in-flight stream.
          isRunning && "bg-blue-600",
          isRunning && "cursor-wait",
          // Disabled-because-another-is-running: gray.
          isAnyRunning && !isRunning && "bg-gray-400",
          isAnyRunning && !isRunning && "cursor-not-allowed",
        )}
      >
        {isRunning ? "Running…" : "Run"}
      </button>

      {output !== undefined && output.length > 0 && (
        <pre
          className={cn(
            "text-xs",
            "font-mono",
            "whitespace-pre-wrap",
            "break-all",
            "opacity-80",
            "p-2",
            "rounded",
            "bg-black/5",
            "dark:bg-white/5",
            "max-h-96",
            "overflow-y-auto",
            "m-0",
          )}
        >
          {output.join("\n")}
        </pre>
      )}
    </article>
  );
}

function formatSort(s: SortBy): string {
  if (typeof s === "string") return s;
  return `custom: ${s.custom}`;
}
