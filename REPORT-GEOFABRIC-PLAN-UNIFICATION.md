# GeoFabric — Plan unification, artifact guard, demo data versioning

Repository <https://github.com/shadowhunter67/geofabric> · Live demo <https://shadowhunter67.github.io/geofabric/> · Data <https://github.com/shadowhunter67/geofabric-demo-data>. Base `f502bbf`; commits `3c38231`, `e8ea1b3`/`6c6dfb1`, `3ce06c3` and the final report commit (see Repository State). 2026-10-23. Four goals from the brief: (1) one request→PlanGraph builder, (2) keep generated artifacts out of Git, (3) move demo data out of normal history, (4) keep the public demo and the local full stack working. The PostGIS cost model was not started.

## Summary

* **Request→plan is built once.** `User request → canonical request model → geo-plan (Rust) → PlanGraph → geo-lowering → DuckDB | PostGIS | Browser`. The gateway links `geo-plan` natively; the static page runs the same code as WebAssembly inside the lowering module it already loads. `web/src/static/ir.ts` and `geo-gateway/src/templates.rs` are deleted; `web/src/static/adapter.ts` is a thin adapter.
* **Generated artifacts cannot be committed again.** `tools/repo_guard.py` (forbidden build dirs, 20 MiB limit, binary types with a reasoned allowlist) is the first CI job; every other job `needs` it. `.gitignore` hardened; optional `.githooks/pre-commit`.
* **Demo data left the code history.** v1 (same bytes) is published to a new public repository `shadowhunter67/geofabric-demo-data` under the ODbL 1.0; the manifest points there; v1 was removed from this tree (history untouched; the old commit-pinned URLs still answer 206). The tracked tree went from 41.3 MiB to 15.0 MiB (largest file 13.1 MiB → 0.9 MiB).
* **Nothing regressed**: 17/17 cross-engine plans identical, Demo B 27,624 ids on both engines, 8/8 live PostGIS driver tests, full-stack Chrome and Edge green, live static green on Chrome and Edge, CI and Pages green.

## 1. Plan unification

| Item | Result |
|---|---|
| Request model | `geo_plan::PlanRequest` (+ `FilterRequest`, `WithRequest`), `REQUEST_SCHEMA_VERSION`; no SQL, no engine detail. The engine choice is a planner hint attached after the plan is built and never changes the nodes (fixture `engine_fields_are_not_part_of_the_plan`). |
| Builder | `crates/geo-plan` (pure; arch_check enforces: no engine, no IO). Validates first, then builds. |
| Error contract | `PlanError { code, message }` with `INVALID_FIELD`, `INVALID_FILTER`, `INVALID_BBOX`, `INVALID_CRS`, `UNKNOWN_DATASET`, `UNSUPPORTED_REQUEST`…; HTTP status + legacy error string preserved (`legacy_error()`); no wasm panics (every failure is a JSON error). 3 old gateway refusals deliberately became structured `INVALID_FIELD` (422). |
| Gateway | `QueryReq` flattens `PlanRequest`; `build_query_plan` authorizes sources, opens the Source nodes and calls `build_plan`. It may not construct plan nodes (arch_check). |
| Browser | `gf_build_plan` / `gf_browser_ops` in the existing wasm; `adapter.ts` forwards the request, attaches the hint, reads the capability verdict. Gating of Demo B/C/E derives from the plan (`/api/query/capability`), not a hand-kept list. |
| Server-only operations | Built exactly (transform, spatial semi join, intersects…) and refused by the capability layer with the missing operations named; never approximated, never dropped. |
| Parity evidence | 40 golden request fixtures: native plan JSON == wasm plan JSON, byte for byte (3 tests in `geo-lowering/tests/request_plans.rs`, `web/test/plan.test.ts`). Old-gateway baseline: 19/19 plans byte-identical after the refactor. |
| IR | No change (0.3). Request schema version added. |
| RFC-004 | Accepted; section "Request → plan" added; the debt line removed. |

**Wasm bundle impact** (`geo_lowering.wasm`, one module reused, opt-level z + LTO): 346 KB → 587 KB raw (+241 KB), 102 KB → 164 KB gzipped (+62 KB). Context: DuckDB-Wasm is ~35 MB per variant. The JS side shrank (ir.ts deleted). The growth is the price of carrying the plan model (serde, types, stats) into the browser; it is not hidden.

**Timing** (node 24, the real wasm, 500 iterations after warm-up): plan build 152–184 µs for 3–5-node plans; browser lowering 36–42 µs. First view on the live site (cold context, Chrome, this network, tiles blocked): 18 s, 35 s, 70 s, 18 ranged `206` responses each. The spread is network (35 MB DuckDB-Wasm + extension download); plan build and lowering are four orders of magnitude below it. Not a regression measurement: the previous phase measured a different quantity (see its report).

## 2. Artifact and size guard

`tools/repo_guard.py` over `git ls-files` (or `--staged`): forbidden directories (`dist/`, `dist-*/`, `target/`, `node_modules/`, `.runtime/`, `coverage/`, `playwright-report/`, `test-results/`, `__pycache__/`, `.venv/`, `.cache/`, `pgdata/`), size > 20 MiB, binary/generated extensions outside `tools/repo_guard.allow.json` (each entry needs a reason; stale entries fail), logs > 1 MiB. Current tree: 596 files, 15.0 MiB, largest 0.9 MiB, OK. Negative test: a staged `web/dist-guardtest/a.txt` is flagged. CI: first job `guard`, all others `needs: guard`. History was not rewritten.

## 3. Public demo data versioning

* **Decision: a dedicated data repository** (zero cost, no secrets). Release assets were not used (redirect + CORS/range fail, ADR-014). Licence: the official ODbL 1.0 text (`LICENSE`), `ATTRIBUTION.md` generated from the catalog (verbatim attributions; places keep per-record licences), `README.md`, and per version `SHA256SUMS` + `MANIFEST.json`, tag `v1`.
* **Pipeline** (`data/scripts/publish_demo_data.py`): generate (`make_public_demo.py` → git-ignored `data/public-demo-build/`, with IR source nodes) → validate (size, SHA-256, Parquet framing, metadata, rows/schema read back by the Rust reader) → checksums → one commit + tag per immutable version (an existing version must be byte-identical) → verify range/CORS on the commit-pinned URLs → only then write the manifest. `fetch_demo_data.py` downloads + verifies the data for CI/dev bundling (static CI no longer depends on the CDN).
* **Range status**: `gcore.jsdelivr.net/gh/shadowhunter67/geofabric-demo-data@5a88958…/v1/…` — HEAD/GET Range → 206, `Content-Range`, no `Content-Encoding`, CORS, Parquet magic, SHA-256 of the served bytes: 5/5 files. Two first attempts failed honestly and left the manifest untouched: a read timeout (verifier now retries network errors) and a cold-edge 404/200 on the 13.7 MB file (passed on retry once the edge had it). Old v1 URLs (`geofabric@851b570…`) still answer 206.
* **Switch order** respected: publish → verify → manifest → remove from tree.
* Docs: `docs/public-demo.md` (new), README, DATA-LICENSES, ADR-014 note, deployment-profiles, CONTRIBUTING.

## 4. Regression results

| Check | Result |
|---|---|
| Rust workspace (`--no-default-features`) | 254 passed, 0 failed |
| geo-lowering / 11 lowering fixtures / request fixtures | pass (fixtures: native ↔ wasm byte-identical) |
| Web unit tests | 137 passed; `tsc` clean |
| arch_check, repo_guard | OK |
| Live PostGIS (PG 16.9 + PostGIS 3.6.2, started for the run, stopped after) | 8/8 driver tests; 17/17 cross-engine plans IDENTICAL; Demo B 27,624 ids equal on both engines |
| DuckDB profiler | opt-in contract unit-tested (`profiling_contract`); documented in `docs/postgis.md`/CLAUDE.md |
| Full stack, fresh release gateway, Chrome | 58 passed + 3 re-run (see below) = 61 passed, 4 skipped (opt-in probes) |
| Full stack, Edge | same |
| Static, local (range-capable and whole-file bundles) | 13 / 12 passed |
| Live static suite, Chrome | 13/13 (12 + the one test below re-run after the fix), 1 skipped |
| Live static suite, Edge | 13/13, 1 skipped |
| CI (guard, architecture, web, rust, duckdb repro matrix, experiments) | all success on `3ce06c3` |
| Pages deploy | success on `6c6dfb1` and `3ce06c3` |

Honest notes: (a) the first full-stack runs showed 3 failures in `engines.spec.ts` on both browsers: my shell had `GEOFABRIC_POSTGIS_*` exported from the PostGIS helper, so the gateway reported PostGIS as configured; with a clean environment all 10 tests in that file pass on Chrome and Edge. (b) The first live static run failed one test on both browsers: its route pattern assumed the old `demo/v1/…` URL, and the new data URL is `…geofabric-demo-data@<sha>/v1/…`, so the abort never matched — a test bug from this migration, fixed in `static.spec.ts` and re-run green live. (c) A first local static run failed everything because Git Bash rewrote `GF_BASE=/geofabric/` into a Windows path (documented: `MSYS_NO_PATHCONV=1`), and a second one exposed a real bug: the static gateway planned before the plan-builder wasm was loaded (`ensureLowering` is now awaited through `deps.ready`).

## Architecture risks

1. The data host is a third party CDN over a GitHub repo (jsDelivr): availability is outside our control; the layer degrades with a plain message (tested) and `fetch_demo_data.py` can rebuild a self-contained site from GitHub.
2. First-view time is dominated by DuckDB-Wasm (35 MB) and the extension fetch, not by anything in this phase.
3. The wasm grew by 241 KB; if the request model keeps growing, split the planner from the lowering module.
4. Cold CDN edges can answer 404/200 for minutes after a publish; the pipeline waits and retries, but a manifest switch needs a patient run.

## Remaining P0/P1

* P0: none known.
* P1: PostGIS cost model (explicitly not started); relational skeletons (CTE chains) are still per engine (RFC-004 debt); consider shipping the DuckDB `parquet` extension instead of fetching from `extensions.duckdb.org`.

## Recommended next phase

PostGIS cost model on top of the now-shared request→plan→lowering semantics, with selectivity inputs and the measured engine crossovers (PostGIS wins small indexed windows, DuckDB wins large metric semi-joins).

## Repository state

Normal commits on `main`, no history rewrite, no force push. Tag `v1` in the data repository.
