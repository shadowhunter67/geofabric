#!/usr/bin/env python3
"""Run the held-out planner evaluation for one or more network profiles.

For each profile: start a release gateway with
    --simulate-network <profile>   every /api response is delayed like a link with that RTT / bandwidth
    --telemetry results/calibration-set.jsonl --no-learn   the planner knows the CALIBRATION set and learns nothing more
run the Playwright spec (real Chrome, real DuckDB-Wasm) and stop the gateway again (by PID).

Usage: python benchmarks/run_heldout.py LOCAL FAST_LAN GOOD_BROADBAND MOBILE HIGH_LATENCY
Needs: target/release/geo-gateway.exe, web/dist, benchmarks/data, results/heldout-cases.json, results/calibration-set.jsonl,
DUCKDB_LIB_DIR on PATH (prebuilt libduckdb).
"""
import os
import pathlib
import subprocess
import sys
import time
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parent.parent
PORT = 8810


def wait_health(port, seconds=60):
    for _ in range(seconds * 4):
        try:
            urllib.request.urlopen(f"http://127.0.0.1:{port}/api/health", timeout=1).read()
            return True
        except Exception:
            time.sleep(0.25)
    return False


def main():
    nets = sys.argv[1:] or ["LOCAL"]
    env = dict(os.environ)
    lib = ROOT / "tools" / ".cache" / "duckdb-1.5.6" / "lib"
    env["PATH"] = str(lib) + os.pathsep + env.get("PATH", "")
    for net in nets:
        out = ROOT / "benchmarks" / "results" / f"{os.environ.get('HO_PREFIX', 'heldout')}-{net}.json"
        gw = subprocess.Popen(
            [str(ROOT / "target" / "release" / "geo-gateway.exe"), "--data", "benchmarks/data", "--web", "web/dist", "--port", str(PORT),
             "--simulate-network", net, "--telemetry", "benchmarks/results/calibration-set.jsonl", "--no-learn"],
            cwd=ROOT, env=env, stdout=open(ROOT / "benchmarks" / "results" / f"gw-heldout-{net}.log", "w"), stderr=subprocess.STDOUT,
        )
        try:
            if not wait_health(PORT):
                raise SystemExit(f"gateway for {net} did not start")
            e = dict(env, HELDOUT="1", HO_BASE=f"http://127.0.0.1:{PORT}", HO_NET=net,
                     HO_CASES=str(ROOT / "benchmarks" / "results" / os.environ.get("HO_CASES_FILE", "heldout-cases.json")), HO_OUT=str(out))
            print(f"=== {net}: {out.name}", flush=True)
            subprocess.run(["npx.cmd", "playwright", "test", "-c", "playwright.heldout.config.ts"], cwd=ROOT / "web", env=e, check=False)
        finally:
            gw.terminate()
            try:
                gw.wait(timeout=10)
            except subprocess.TimeoutExpired:
                gw.kill()


if __name__ == "__main__":
    main()
