#!/usr/bin/env python3
"""Generic dev bridge for the psychological-operations-browser.

Spawns the browser with arbitrary pass-through args (after `--`) and keeps it
running, so a caller can drive it across turns:

  * append one JSON `Request` per line to `<workdir>/cmd`  -> child stdin
    e.g.  {"type":"html"}   {"type":"eval","code":"document.title"}
  * read the child's JSONL `Output`/`Response` stream from `<workdir>/out.jsonl`
  * stderr -> `<workdir>/err.log`

Run it in the background; then poke `cmd` and tail `out.jsonl` interactively.

Example:
  python browser-bridge.py \
    --state-dir .browser-bridge/state \
    --postgres-url postgresql://postgres:objectiveai@127.0.0.1:PORT/objectiveai \
    --workdir .browser-bridge \
    -- --discord-login mytag
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
        "--state-dir",
        default=os.environ.get("OBJECTIVEAI_STATE_DIR"),
        help="state root (default: $OBJECTIVEAI_STATE_DIR)",
    )
    ap.add_argument("--binary", default=str(DEFAULT_BINARY))
    ap.add_argument("--workdir", default=str(REPO_ROOT / ".browser-bridge"))
    ap.add_argument(
        "browser_args",
        nargs=argparse.REMAINDER,
        help="args passed verbatim to the browser (put them after `--`), "
        "e.g. -- --discord-login mytag",
    )
    args = ap.parse_args()

    if not args.state_dir:
        ap.error("--state-dir is required (or set OBJECTIVEAI_STATE_DIR)")

    # argparse.REMAINDER keeps a leading `--`; drop it.
    passthrough = [a for a in args.browser_args if a != "--"]
    if not passthrough:
        ap.error("no browser args given (put a mode after `--`, e.g. -- --discord-login mytag)")

    workdir = pathlib.Path(args.workdir)
    workdir.mkdir(parents=True, exist_ok=True)
    cmd_path = workdir / "cmd"
    out_path = workdir / "out.jsonl"
    err_path = workdir / "err.log"

    cmd_path.write_text("", encoding="utf-8")
    out_f = out_path.open("w", encoding="utf-8")
    err_f = err_path.open("w", encoding="utf-8")

    env = dict(os.environ)

    cmd = [args.binary, "--state-dir", args.state_dir, *passthrough]
    print(f"[bridge] spawning: {' '.join(cmd)}", flush=True)
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
    threading.Thread(target=pump_stream, args=(proc.stderr, [err_f]), daemon=True).start()

    print(f"[bridge] driving via {cmd_path} -> child stdin; output -> {out_path}", flush=True)
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
