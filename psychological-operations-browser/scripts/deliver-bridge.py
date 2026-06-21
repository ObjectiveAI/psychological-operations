#!/usr/bin/env python3
"""Dev bridge for iterating on reply/quote delivery against the live x.com DOM.

NOT shipped browser code — a local harness so a long-lived
`psychological-operations-browser --deliver ...` process can be driven across
separate shell invocations (e.g. one Claude tool call per turn).

It spawns the browser with stdin/stdout piped, then:
  * pumps the child's stdout (the JSON-Lines `Output` protocol) into OUT_FILE
    (and echoes to this process's stdout), child stderr into ERR_FILE;
  * tails CMD_FILE — every line appended there is written to the child's stdin
    as a `Request` (e.g. {"type":"html"}, {"type":"eval","code":"..."},
    {"type":"shutdown"}). CMD_FILE is truncated on startup so stale commands
    from a previous run don't replay.

Typical use (run the bridge in the background, then drive it):
    python deliver-bridge.py \
        --state-dir "$OBJECTIVEAI_STATE_DIR" \
        --postgres-url "$OBJECTIVEAI_POSTGRES_URL" \
        --deliver '[{"tweet_id":"...","agent":"alice","content":"hi","kind":"reply"}]'

  # introspect the live DOM from another shell:
    echo '{"type":"html"}'                              >> .deliver-bridge/cmd
    echo '{"type":"eval","code":"document.title"}'      >> .deliver-bridge/cmd
    tail -n 40 .deliver-bridge/out.jsonl

  # clean shutdown:
    echo '{"type":"shutdown"}' >> .deliver-bridge/cmd
"""

import argparse
import os
import pathlib
import subprocess
import sys
import threading
import time

REPO_ROOT = pathlib.Path(__file__).resolve().parents[2]
DEFAULT_BINARY = REPO_ROOT / "target" / "debug" / "psychological-operations-browser.exe"
DEFAULT_WORKDIR = pathlib.Path(".deliver-bridge")


def pump_stream(stream, sinks):
    """Copy a child stream line-by-line into every sink (flushing each)."""
    for raw in iter(stream.readline, b""):
        line = raw.decode("utf-8", errors="replace")
        for sink in sinks:
            sink.write(line)
            sink.flush()
    stream.close()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument(
        "--agent-deliver",
        required=True,
        help="agent tag to deliver as (passed verbatim to --agent-deliver)",
    )
    ap.add_argument(
        "--agent-deliver-items",
        required=True,
        help="inline JSON array of DeliverItem {tweet_id, content, kind} (passed verbatim to --agent-deliver-items)",
    )
    ap.add_argument(
        "--state-dir",
        default=os.environ.get("OBJECTIVEAI_STATE_DIR"),
        help="state root (default: $OBJECTIVEAI_STATE_DIR)",
    )
    ap.add_argument(
        "--postgres-url",
        default=os.environ.get("OBJECTIVEAI_POSTGRES_URL"),
        help="postgres URL (default: $OBJECTIVEAI_POSTGRES_URL)",
    )
    ap.add_argument("--binary", default=str(DEFAULT_BINARY))
    ap.add_argument("--workdir", default=str(DEFAULT_WORKDIR))
    args = ap.parse_args()

    if not args.state_dir:
        ap.error("--state-dir is required (or set OBJECTIVEAI_STATE_DIR)")
    if not args.postgres_url:
        ap.error("--postgres-url is required (or set OBJECTIVEAI_POSTGRES_URL)")

    workdir = pathlib.Path(args.workdir)
    workdir.mkdir(parents=True, exist_ok=True)
    cmd_path = workdir / "cmd"
    out_path = workdir / "out.jsonl"
    err_path = workdir / "err.log"

    # Truncate so a previous run's commands/output don't bleed in.
    cmd_path.write_text("", encoding="utf-8")
    out_f = out_path.open("w", encoding="utf-8")
    err_f = err_path.open("w", encoding="utf-8")

    env = dict(os.environ)
    env["OBJECTIVEAI_POSTGRES_URL"] = args.postgres_url

    cmd = [
        args.binary,
        "--state-dir",
        args.state_dir,
        "--agent-deliver",
        args.agent_deliver,
        "--agent-deliver-items",
        args.agent_deliver_items,
    ]
    print(f"[bridge] spawning: {cmd[0]} --state-dir {args.state_dir} "
          f"--agent-deliver {args.agent_deliver} "
          f"--agent-deliver-items <{len(args.agent_deliver_items)} bytes>",
          flush=True)
    proc = subprocess.Popen(
        cmd,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=env,
        cwd=str(REPO_ROOT),
    )

    threading.Thread(
        target=pump_stream, args=(proc.stdout, [sys.stdout, out_f]), daemon=True
    ).start()
    threading.Thread(
        target=pump_stream, args=(proc.stderr, [err_f]), daemon=True
    ).start()

    # Tail CMD_FILE: forward each newly-appended line to the child's stdin.
    print(f"[bridge] driving via {cmd_path} -> child stdin; output -> {out_path}",
          flush=True)
    offset = 0
    try:
        while proc.poll() is None:
            try:
                size = cmd_path.stat().st_size
            except FileNotFoundError:
                size = 0
            if size > offset:
                with cmd_path.open("r", encoding="utf-8") as fh:
                    fh.seek(offset)
                    chunk = fh.read()
                    offset = fh.tell()
                for line in chunk.splitlines():
                    line = line.strip()
                    if not line:
                        continue
                    print(f"[bridge] -> {line}", flush=True)
                    try:
                        proc.stdin.write((line + "\n").encode("utf-8"))
                        proc.stdin.flush()
                    except (BrokenPipeError, OSError) as e:
                        print(f"[bridge] stdin closed: {e}", flush=True)
                        break
            elif size < offset:
                # File was truncated/rotated — restart from the top.
                offset = 0
            time.sleep(0.2)
    except KeyboardInterrupt:
        pass
    finally:
        rc = proc.poll()
        print(f"[bridge] child exited rc={rc}", flush=True)
        out_f.flush()
        err_f.flush()


if __name__ == "__main__":
    main()
