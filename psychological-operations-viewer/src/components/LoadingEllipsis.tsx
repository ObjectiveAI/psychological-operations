// NB: this codebase uses `classnames` with one tailwind class per
// comma-separated string — never multi-class strings. Conditional
// classes use `&&` short-circuit in line with the rest of the call.
import cn from "classnames";

export function LoadingEllipsis() {
  return (
    <span
      className={cn("inline-flex", "items-center", "gap-1", "text-lg")}
      aria-label="Loading"
    >
      <span>Loading</span>
      <span
        className={cn("animate-bounce", "inline-block")}
        style={{ animationDelay: "0ms" }}
      >
        .
      </span>
      <span
        className={cn("animate-bounce", "inline-block")}
        style={{ animationDelay: "150ms" }}
      >
        .
      </span>
      <span
        className={cn("animate-bounce", "inline-block")}
        style={{ animationDelay: "300ms" }}
      >
        .
      </span>
    </span>
  );
}
