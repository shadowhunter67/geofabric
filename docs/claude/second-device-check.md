# Second physical device check (the one open item of the planner gate)

Why: ADR-010 may only become Accepted with timing/correctness evidence from a DIFFERENT physical machine (same-machine Edge/WebKit/Firefox do not count). Everything else of the planner gate is closed (REPORT-GEOFABRIC-FINAL-PLANNER-VALIDATION.md).

## Option A — the other laptop only needs a browser (recommended, nothing to install there)
On THIS machine (explicit user approval needed before exposing a port; bind to the LAN IP only, never 0.0.0.0; the gateway has no authentication):
```
# from the repo root, PowerShell, DuckDB lib on PATH
$env:PATH = "$PWD\tools\.cache\duckdb-1.5.6\lib;$env:PATH"
.\target\release\geo-gateway.exe --data data/local --data benchmarks/data --manifests data/manifests --repo-root . --web web/dist `
  --host <this machine's LAN IP> --port 8787 --devicecheck-dir benchmarks/results/devicecheck --no-learn
```
Allow the Windows Firewall prompt for the private network. On the OTHER laptop open `http://<LAN IP>:8787/?devicecheck=1` (Chrome or Edge), leave the tab in the foreground until it prints "done". The report is saved here as `benchmarks/results/devicecheck/device-<start time>.json` (it is posted after every case: a crash leaves a trace). Note that this measures the other device's browser against this machine's server over the real LAN (the LAN link is the "constrained" network the brief asks for; for a second profile open the same URL from a hotspot or throttle in DevTools).

Record next to the report (no fingerprinting beyond this): CPU class, RAM class, browser + version, OS, network type. `deviceMemory` and core count are already in the JSON.

Stop the gateway by PID (`netstat -ano | findstr :8787`, then `taskkill /PID <pid> /F`); never by name.

## Option B — run everything on the other laptop
Needs the repo build, `tools/.cache/duckdb-1.5.6`, `benchmarks/data` (~3 GB for the sizes in the matrix: clustered_10mb/30mb/100mb spatial + shuffled, Overture HCM pack) — much more work; use only if the LAN cannot be used.

## What counts as passing
All 7 runnable cases: browser digest = server digest; the `unsafe-candidate` case stays on the server; no tab crash; planner never offers the browser above 128 MiB. Timing is reported, not gated (a different machine will have a different level; compare the browser/server ratio per case with this machine's matrix: `benchmarks/results/devicecheck/matrix-*.json`).
