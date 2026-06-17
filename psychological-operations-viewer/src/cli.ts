import { pluginsRunExecute } from "@objectiveai/sdk/viewer";
import { version } from "../package.json";

// This plugin's install coordinate. owner/name are fixed; the version is
// the viewer bundle's own version — version.sh keeps it in lockstep with
// the CLI + manifest, and the viewer ships in the same GitHub release as
// the plugin the host is serving it for, so it matches the installed copy.
const OWNER = "ObjectiveAI";
const NAME = "psychological-operations";

/**
 * Run a psychological-operations CLI command on the host and stream its
 * typed output lines.
 *
 * Replaces the viewer SDK's old `invokeCli(args)` — raw-argv invocation
 * was removed; `plugins run` lowers the args to a fresh plugin process on
 * the host and routes the output back to this iframe. Each yielded item
 * is a `CliCommandPluginsRunResponseItem`: an `{type:"error",…}` envelope,
 * an `{type:"mcp",…}` announcement, or one of the plugin's own JSON output
 * lines (`{type:"notification"|"ok",…}`) passed through verbatim.
 *
 * The transport has no per-invocation demux, so callers must run these
 * sequentially — never two concurrently from the same iframe.
 */
export function runCli(args: string[]) {
  return pluginsRunExecute({ owner: OWNER, name: NAME, version, args });
}
