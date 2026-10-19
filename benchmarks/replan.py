#!/usr/bin/env python3
"""Re-plan every case of a measured set with the CURRENT planner (one gateway per network profile, by PID) and rebuild the table.
   python replan.py <prefix> <cases.json> <out.json> [--port 8831]
Nothing is re-measured: measured columns come from results/<prefix>-<NET>.json; planner columns from the live gateway."""
import json, os, subprocess, sys, time, urllib.request, pathlib
ROOT = pathlib.Path(__file__).resolve().parent.parent
prefix, cases, out = sys.argv[1], str(pathlib.Path(sys.argv[2]).resolve()), sys.argv[3]
PORT = int(sys.argv[sys.argv.index("--port") + 1]) if "--port" in sys.argv else 8831
NETS = ["LOCAL", "FAST_LAN", "GOOD_BROADBAND", "MOBILE", "HIGH_LATENCY"]
lib = ROOT / "tools" / ".cache" / "duckdb-1.5.6" / "lib"
env = dict(os.environ, PATH=str(lib) + os.pathsep + os.environ["PATH"])

def up():
    try:
        urllib.request.urlopen(f"http://127.0.0.1:{PORT}/api/state", timeout=2); return True
    except Exception:
        return False

if up(): sys.exit(f"port {PORT} already served; stop it by PID")
rows = []
for net in NETS:
    gw = subprocess.Popen([str(ROOT / "target/release/geo-gateway.exe"), "--data", "data/local", "--data", "benchmarks/data", "--manifests", "data/manifests", "--repo-root", ".", "--web", "web/dist", "--port", str(PORT),
                           "--network", net, "--telemetry", "benchmarks/results/calibration-v3.jsonl", "--no-learn"], cwd=ROOT, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        for _ in range(600):
            try:
                st = json.load(urllib.request.urlopen(f"http://127.0.0.1:{PORT}/api/state", timeout=2))
                if st["histograms"]["pending"] == 0: break
            except Exception: pass
            time.sleep(1)
        tmp = ROOT / "benchmarks" / "results" / f"_replan-{net}.json"
        subprocess.run([sys.executable, "browsercal_table.py", f"http://127.0.0.1:{PORT}", prefix, cases, str(tmp)], cwd=ROOT / "benchmarks", check=True, env=dict(env, BCT_NETS=net, PYTHONIOENCODING="utf-8"))
        rows += json.load(open(tmp)); tmp.unlink()
    finally:
        subprocess.run(["taskkill", "/PID", str(gw.pid), "/F"], capture_output=True)
        gw.wait()
json.dump(rows, open(out, "w"), indent=1); print(len(rows), "rows ->", out)
