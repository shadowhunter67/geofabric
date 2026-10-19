#!/usr/bin/env python3
"""Run the combined Playwright suite N times and keep evidence of every failure (planner hardening III, "flake investigation").

Per iteration: start time, browser, gateway port and the PID that held it, the JSON report of Playwright; per failure: test title,
file, duration, error, and the shared state that could explain it (health state of the engines and the telemetry size read from
/api/state right after the run, because the gateway keeps health between suites that share one server).

  python benchmarks/flake_loop.py --runs 20 [--channel chrome|msedge] [--specs smoke explorer ...]

Never kills anything by name: the gateway is started and stopped by Playwright (webServer); this script only reads.
"""
import argparse
import datetime
import json
import os
import pathlib
import subprocess
import sys
import time
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parent.parent
WEB = ROOT / "web"
PORT = 8798
DEFAULT_SPECS = ["smoke", "explorer", "engines", "planner", "map", "data-pack", "sampling"]


def listener_pid(port):
    out = subprocess.run(["netstat", "-ano"], capture_output=True, text=True).stdout
    for line in out.splitlines():
        if f":{port} " in line and "LISTENING" in line:
            return int(line.split()[-1])
    return None


def state():
    try:
        with urllib.request.urlopen(f"http://127.0.0.1:{PORT}/api/state", timeout=2) as r:
            s = json.load(r)
        return {"health": {k: v.get("state") for k, v in s.get("health", {}).items()}, "telemetry": s.get("telemetry")}
    except Exception as e:  # server already gone: Playwright stops it with the run
        return {"unavailable": str(e)[:80]}


def walk(suite, path=()):
    for spec in suite.get("specs", []):
        for t in spec.get("tests", []):
            for r in t.get("results", []):
                yield spec["file"], spec["title"], r
    for s in suite.get("suites", []):
        yield from walk(s)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--runs", type=int, default=20)
    ap.add_argument("--channel", default="chrome")
    ap.add_argument("--specs", nargs="*", default=DEFAULT_SPECS)
    ap.add_argument("--out", default=str(ROOT / "benchmarks" / "results" / "flake-loop.json"))
    a = ap.parse_args()
    if listener_pid(PORT):
        sys.exit(f"port {PORT} is already served (PID {listener_pid(PORT)}): stop it first (by PID), the runs would reuse a stale gateway")
    env = dict(os.environ, PW_CHANNEL=a.channel, PLAYWRIGHT_JSON_OUTPUT_NAME=str(ROOT / "benchmarks" / "results" / "flake-last.json"))
    lib = ROOT / "tools" / ".cache" / "duckdb-1.5.6" / "lib"
    env["PATH"] = str(lib) + os.pathsep + env["PATH"]
    env["DUCKDB_LIB_DIR"] = str(lib)
    log = []
    for i in range(1, a.runs + 1):
        t0 = time.time()
        started = datetime.datetime.now().isoformat(timespec="seconds")
        p = subprocess.Popen(["npx.cmd", "playwright", "test", *a.specs, "--reporter=json"], cwd=WEB, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        pid = None
        st = {}
        while p.poll() is None:
            time.sleep(5)
            pid = pid or listener_pid(PORT)
            if time.time() - t0 > 60:
                st = state() or st  # last readable state before the server stops
        p.wait()
        report = json.loads(pathlib.Path(env["PLAYWRIGHT_JSON_OUTPUT_NAME"]).read_text(encoding="utf-8"))
        failures = []
        n = 0
        for file, title, r in walk({"suites": report.get("suites", [])}):
            n += 1
            if r.get("status") not in ("passed", "skipped"):
                failures.append({"file": file, "test": title, "status": r.get("status"), "duration_ms": r.get("duration"), "error": (r.get("error", {}) or {}).get("message", "")[:600]})
        entry = {"iteration": i, "started": started, "seconds": round(time.time() - t0), "browser": a.channel, "port": PORT, "gateway_pid": pid,
                 "tests": n, "failed": len(failures), "failures": failures, "shared_state_last_seen": st}
        log.append(entry)
        pathlib.Path(a.out).write_text(json.dumps({"runs": log}, indent=1), encoding="utf-8")
        print(f"[{i}/{a.runs}] {started} tests={n} failed={len(failures)} pid={pid} {round(time.time() - t0)}s", flush=True)
        for f in failures:
            print("   FAIL", f["file"], f["test"], f["error"][:120].replace("\n", " "), flush=True)
    bad = sum(1 for e in log if e["failed"])
    print(f"done: {a.runs} runs, {bad} with failures")
    sys.exit(0 if bad == 0 else 1)


if __name__ == "__main__":
    main()
