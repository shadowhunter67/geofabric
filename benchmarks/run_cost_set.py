"""Run one benchmark set with a resume loop, a record of the host's resource watchdog, and an honest classification of every abnormal exit.

On this machine a RAM emergency killer (C:\\Users\\Admin\\Scripts\\ram-emergency-killer.ps1) terminates the heaviest process when memory use passes 90 %.
Twice a benchmark "crash" (exit 0xffffffff) was exactly that, and was first mistaken for an engine fault. This wrapper makes the distinction mechanical:

  HOST_RESOURCE_KILL   the watchdog's log names the benchmark process in the window of the run
  ENGINE_CRASH         a non-zero exit with no watchdog entry
  OK                   exit 0

A host-killed attempt is retried (the harness resumes after the last finished case) and its cases are NEVER fitted differently: the case that was
running simply has no record until it finishes in a later attempt. The metadata is written next to the results (`<out>.meta.json`).

    python benchmarks/run_cost_set.py --cases cases-x.json --out raw/x.jsonl [--attempts 4] [-- <extra env as K=V ...>]
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
KILLER_LOG = Path(os.environ.get("RAM_KILLER_LOG", r"C:\Users\Admin\Scripts\ram-emergency-killer.log"))


def killer_state() -> dict:
    ps = ["powershell", "-NoProfile", "-Command",
          "Get-CimInstance Win32_Process -Filter \"Name='powershell.exe'\" | Where-Object { $_.CommandLine -match 'ram-emergency' -and $_.CommandLine -match '-File' } | ForEach-Object { $_.ProcessId }"]
    out = subprocess.run(ps, capture_output=True, text=True).stdout.split()
    return {"active": bool(out), "pids": [int(p) for p in out], "log_exists": KILLER_LOG.exists()}


def log_size() -> int:
    return KILLER_LOG.stat().st_size if KILLER_LOG.exists() else 0


def killer_lines_since(offset: int) -> list[str]:
    if not KILLER_LOG.exists():
        return []
    with KILLER_LOG.open("rb") as f:
        f.seek(offset)
        return [l.strip() for l in f.read().decode("utf-8", "replace").splitlines() if l.strip()]


def classify(exit_code: int, new_lines: list[str]) -> str:
    if exit_code == 0:
        return "OK"
    return "HOST_RESOURCE_KILL" if any("postgis_cost" in l and "killed" in l for l in new_lines) else "ENGINE_CRASH"


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--cases", type=Path, required=True)
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--attempts", type=int, default=4)
    ap.add_argument("--log", type=Path)
    ap.add_argument("--env", nargs="*", default=[], help="extra K=V environment entries (GF_COST_*)")
    a = ap.parse_args()
    env = dict(os.environ)
    env.update(dict(kv.split("=", 1) for kv in a.env))
    env["GF_COST_CASES"] = str(a.cases.resolve())
    env["GF_COST_OUT"] = str(a.out.resolve())
    git = lambda *c: subprocess.run(["git", *c], cwd=ROOT, capture_output=True, text=True).stdout.strip()
    meta = {
        "cases": a.cases.name, "out": a.out.name, "commit": git("rev-parse", "HEAD"), "dirty": bool(git("status", "--porcelain")),
        "watchdog_before": killer_state(), "started": time.strftime("%Y-%m-%dT%H:%M:%S"), "attempts": [],
    }
    cmd = ["cargo", "test", "--release", "-p", "geo-runtime", "--no-default-features", "--features", "postgis-live", "--test", "postgis_cost", "--", "--nocapture"]
    log = a.log or a.out.with_suffix(".log")
    for attempt in range(1, a.attempts + 1):
        offset = log_size()
        t0 = time.time()
        with log.open("ab") as lf:
            rc = subprocess.run(cmd, cwd=ROOT, env=env, stdout=lf, stderr=subprocess.STDOUT).returncode
        lines = killer_lines_since(offset)
        kind = classify(rc, lines)
        meta["attempts"].append({"attempt": attempt, "exit": rc, "class": kind, "seconds": round(time.time() - t0), "watchdog_lines": [l for l in lines if "postgis_cost" in l or "RESOLVED" in l][:6]})
        print(f"attempt {attempt}: exit {rc} -> {kind} ({round(time.time() - t0)} s)")
        if kind == "OK":
            break
        if kind == "ENGINE_CRASH":
            print("NOT a host kill: stopping instead of retrying (investigate the log)")
            break
    meta["finished"] = time.strftime("%Y-%m-%dT%H:%M:%S")
    meta["result"] = meta["attempts"][-1]["class"]
    a.out.with_suffix(".meta.json").write_text(json.dumps(meta, indent=2) + "\n", encoding="utf-8")
    return 0 if meta["result"] == "OK" else 1


if __name__ == "__main__":
    sys.exit(main())
