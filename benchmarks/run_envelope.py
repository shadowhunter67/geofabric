#!/usr/bin/env python3
"""Browser safe-envelope run: python benchmarks/run_envelope.py [chrome|msedge]  (gateway with a long timeout, no network simulation)."""
import os, pathlib, subprocess, sys, time, urllib.request
ROOT = pathlib.Path(__file__).resolve().parent.parent
channel = sys.argv[1] if len(sys.argv) > 1 else "chrome"
env = dict(os.environ)
env["PATH"] = str(ROOT / "tools" / ".cache" / "duckdb-1.5.6" / "lib") + os.pathsep + env.get("PATH", "")
gw = subprocess.Popen([str(ROOT / "target" / "release" / "geo-gateway.exe"), "--data", "benchmarks/data", "--web", "web/dist", "--port", "8810", "--timeout-secs", "180", "--no-learn"],
                      cwd=ROOT, env=env, stdout=open(ROOT / "benchmarks/results/gw-envelope.log", "w"), stderr=subprocess.STDOUT)
try:
    for _ in range(240):
        try:
            urllib.request.urlopen("http://127.0.0.1:8810/api/health", timeout=1).read(); break
        except Exception:
            time.sleep(0.25)
    e = dict(env, ENVELOPE="1", HO_BASE="http://127.0.0.1:8810", HO_CHANNEL=channel, EN_OUT=str(ROOT / f"benchmarks/results/envelope-{channel}.json"))
    subprocess.run(["npx.cmd", "playwright", "test", "-c", "playwright.heldout.config.ts", "envelope"], cwd=ROOT / "web", env=e)
finally:
    gw.terminate()
