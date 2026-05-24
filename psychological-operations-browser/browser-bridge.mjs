#!/usr/bin/env node
// Spawns the psychological-operations-browser binary and exposes its
// stdin via a Windows named pipe so other processes can send commands
// to it on demand.
//
// Usage:
//   node browser-bridge.mjs run [browser-args...]
//   node browser-bridge.mjs send '<json-line>'   # one-shot writer
//
// The "run" subcommand stays in the foreground. Browser stdout and
// stderr are mirrored to this process's stdout/stderr so JSONL events
// are visible. Ctrl-C kills both the bridge and the browser.

import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import net from "node:net";
import path from "node:path";

const PIPE_NAME = "psyops_browser_stdin";
const PIPE_PATH = `\\\\.\\pipe\\${PIPE_NAME}`;

const SCRIPT_DIR = path.dirname(fileURLToPath(import.meta.url));
const WORKSPACE_ROOT = path.resolve(SCRIPT_DIR, "..");
const DEFAULT_BINARY = path.join(
  WORKSPACE_ROOT,
  "target",
  "debug",
  "psychological-operations-browser.exe",
);

const [, , subcommand, ...rest] = process.argv;

if (subcommand === "send") {
  const line = rest.join(" ");
  if (!line) {
    console.error("usage: browser-bridge.mjs send '<json-line>'");
    process.exit(2);
  }
  const client = net.connect(PIPE_PATH);
  client.on("connect", () => {
    client.end(line.endsWith("\n") ? line : line + "\n");
  });
  client.on("error", (e) => {
    console.error(`[send] pipe error: ${e.message}`);
    process.exit(1);
  });
} else if (subcommand === "run") {
  const child = spawn(DEFAULT_BINARY, rest, {
    stdio: ["pipe", "pipe", "pipe"],
    windowsHide: false,
  });

  child.stdout.on("data", (d) => process.stdout.write(d));
  child.stderr.on("data", (d) => process.stderr.write(d));

  child.on("error", (e) => {
    console.error(`[bridge] spawn error: ${e.message}`);
    process.exit(1);
  });
  child.on("exit", (code, signal) => {
    console.error(`[bridge] browser exited code=${code} signal=${signal}`);
    server.close();
    process.exit(code ?? 0);
  });

  const server = net.createServer((socket) => {
    socket.on("data", (data) => {
      child.stdin.write(data);
    });
    socket.on("error", (e) => {
      console.error(`[bridge] client socket error: ${e.message}`);
    });
  });
  server.on("error", (e) => {
    console.error(`[bridge] pipe server error: ${e.message}`);
    child.kill();
    process.exit(1);
  });
  server.listen(PIPE_PATH, () => {
    console.error(`[bridge] listening on ${PIPE_PATH}`);
  });

  const shutdown = () => {
    child.kill();
    server.close();
  };
  process.on("SIGINT", shutdown);
  process.on("SIGTERM", shutdown);
} else {
  console.error(
    "usage:\n  browser-bridge.mjs run [browser-args...]\n  browser-bridge.mjs send '<json-line>'",
  );
  process.exit(2);
}
