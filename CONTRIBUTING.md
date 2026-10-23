# Contributing

GeoFabric is experimental. Small, evidence-backed changes are welcome. Open an issue before anything that touches the IR, the execution contract, CRS/unit semantics, the planner or a public API (those need an ADR/RFC; see [AGENTS.md](AGENTS.md)).

## Prerequisites

Rust ≥ 1.99, Node ≥ 24, Python ≥ 3.12. Windows is the development platform (the verified prebuilt DuckDB used for development is the Windows archive; CI runs the Rust tests on Windows).

## Install and test

```
# web (also what CI runs)
cd web
npm ci
npm run typecheck && npm test                                  # TypeScript unit tests
GF_BASE=/geofabric/ npm run build:static                       # static browser profile
npx playwright install chromium                                # once
GF_OUT=dist PW_CHANNEL=chrome npx playwright test -c playwright.static.config.ts    # static-mode browser tests

# Rust
python tools/duckdb_prebuilt.py                                # verified libduckdb (sha256 pinned)
export DUCKDB_LIB_DIR="<printed dir>"; export PATH="$DUCKDB_LIB_DIR:$PATH"
cargo test --workspace --no-default-features

# architecture
python tools/arch_check.py
```

Live PostgreSQL/PostGIS tests (`--features live`) need a server and skip otherwise; **a skipped test is not evidence**, report live and offline results separately. The full-stack browser suite needs the Gateway (`cd web && npm run build && npx playwright test`, see the comments in `web/playwright.config.ts`).

## Repository hygiene (checked first in CI)

`python tools/repo_guard.py` fails on tracked build output (`dist*/`, `target/`, `node_modules/`, `.runtime/`...), any file above 20 MiB, and binary/generated file types outside `tools/repo_guard.allow.json` (every exception needs a reason). Optional: `git config core.hooksPath .githooks` runs it on staged files. Data does not belong in this repository: demo data is published to `geofabric-demo-data` (see `docs/public-demo.md`); static tests that bundle data run `python data/scripts/fetch_demo_data.py` first.

## Run it

* Static: `cd web && npm run build:static && node scripts/pages-like-server.mjs dist 4190` (a tiny server that behaves like GitHub Pages; open `http://127.0.0.1:4190/geofabric/` after building with `GF_BASE=/geofabric/`).
* Local full stack: `cargo run -p geo-gateway --no-default-features -- --data benchmarks/data --port 8787` (or `start-ui.bat`), UI development with `cd web && npm run dev`.

## Invariants (a PR that breaks one will not be merged)

1. CRS and units are never implicit. 2. GeoJSON is a boundary format only. 3. The IR does not depend on DuckDB/PostGIS/MapLibre; engines are replaceable adapters; core does not depend on UI. 4. The same logical plan runs in browser, local server and any other engine. 5. Every optimization needs benchmark evidence. 6. Planner decisions are observable. 7. Never silently change output semantics to make a test pass. 8. Metric distance is metres, never degrees. 9. Features a deployment cannot run are refused with a reason, never approximated. `tools/arch_check.py` enforces the dependency rules.

## Process safety

Never terminate processes by executable name (`taskkill /IM node.exe`, `pkill node`, `killall python`, `taskkill /IM postgres.exe` are forbidden): only by a verified PID or an owned handle. Do not change global toolchains. Never read, print or commit `.env` files or credentials; use placeholders (`.env.example`).

## Pull requests

* Small and reviewable; one concern per PR. Add a test for every bug fix.
* State what you ran (live vs offline separately) and what you did not.
* Update the docs/ADR that describe the behaviour you changed. Do not claim more than the evidence supports.
* Large data files, database dumps, binaries and runtime directories are never committed.
