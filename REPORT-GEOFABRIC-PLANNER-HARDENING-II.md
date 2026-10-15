# REPORT — GeoFabric Planner Hardening II + UI Productization

Date 2026-10-15 · Branch `main` · Commit: see the last line of this file · Author: Engineering Lead (Claude Sonnet 5.5)

Full tables (all 100 held-out cases, worst-12 lists, per-network tables): `benchmarks/results/summary-planner-hardening-ii.md`. Raw data: `benchmarks/results/heldout-*.json`.

## 1. Executive Summary

- **Estimator v3 meets its targets.** On a fresh held-out set (100 cases, seed 20261018, run once, no tuning) the cardinality error is median 1.01×, **P90 1.13×**, P95 1.33×, max 2.0×. No estimate was too low by more than 5×; the true value was inside the stated interval in 100 % of cases.
- **The planner is better than "always use the server", and no longer by luck.** Accuracy 97 % against 90 % for always-server; P90 regret 1.00; the browser was chosen in 7 cases and fastest in 10 (all on LOCAL). 0 crashes, 0 digest mismatches (40 checked), 0 unsafe choices.
- **One numeric gate is missed.** The browser time model has median 1.37× (gate 1.25×); per profile, LOCAL (median 1.71×, P90 3.5×) and HIGH_LATENCY (P90 3.4×) exceed the gates. The server model passes (median 1.11×, P90 1.50×).
- **Two earlier held-out runs were invalid and are archived, not used.** v3: the gateway's network *simulation* added a per-request artifact. v4: all five gateways failed to start because a stale gateway still held the port, so every case was planned as MOBILE; the harness did not notice. v5 was generated after the harness fix. This cost most of a day, and it is the most important lesson of the round.
- **UI productization is done** (Explorer / Developer, Vietnamese and English, summary-first execution panel, pipeline strip, accessibility tests).
- **Recommendation: KEEP HARDENING PLANNER** (narrowly). The estimator is ready; the browser time model, a second real device and anything above ~1 GB are not. No PostGIS work was started.

## 2. Estimator v3

Separates **cardinality** (rows matching window and filter) from **output rows** (after aggregate or LIMIT). Inputs: Parquet footer statistics, a 128×128 spatial histogram of bounding-box centres, and per-column statistics (exact category frequencies, a 65-point quantile sketch for numbers). They are built in the background when the gateway starts (about 2 s for 1 GB) and cached in `.gf-cache/`. Filters use the independence assumption (documented as an assumption, its error is visible in the `filt=yes` layout classes below). Group counts: exact frequencies, else history of the *same column*, else a prior.

| held-out v5, n = 100 | median | P90 | P95 | max | over | under | under by >5× | inside interval |
|---|---|---|---|---|---|---|---|---|
| cardinality | 1.01× | 1.13× | 1.33× | 2.0× | 51 % | 36 % | 0 % | 100 % |
| output rows (what the query returns) | 1.00× | 1.13× | 1.33× | 2.0× | 20 % | 7 % | 0 % | 89 % |

`unsafe_underestimate` (an estimate more than 5× too low): **0**. Worst layout classes: `coh=high|skew=peaked|filt=yes` P90 2.0× (n = 8); `count_by` P90 2.0× (n = 26): group counts are the weakest part. Earlier numbers (round I: P90 11.5×, max 29×) came from the old estimator on a different set; they are not directly comparable but explain why the target was missed then.

## 3. Calibration Isolation

Correction lookup order: (source, selectivity bucket) with ≥ 2 runs; else (layout class, bucket) with ≥ 3 runs, where the class is `coherence|skew|covering|histogram|filter`; else **none** (factor 1, wide interval, quality Poor). Interval half-width = the P90 residual of the same context. Count queries are not used to learn selectivity. Leave-one-source-out on the calibration set: P90 1.06×. In v5 all 100 cases had a histogram (`hist=yes`) and a declared covering column, so the "no context" fallback was exercised only by the unit tests, not by held-out data: **an untested path on real data** (see Remaining Risks).

## 4. LIMIT Semantics

A LIMIT is an output cap, not a cardinality. The estimate carries `cardinality` (low, high) and `rows` (after the cap); `limit_certain` is true only when the cardinality *lower* bound is at least the limit. Before, a LIMIT-bound estimate was called "good" on a shuffled file where it was wrong; that case is now a unit test. Features queries: P90 1.08×, max 1.33×.

## 5. Fresh Held-out Set

Seed 20261018 (earlier seeds 20261014/15/16/17 belong to exploratory, v3, and v4). 100 cases: 6 clustered/uniform × 10/30/100 MB × spatial/shuffled files plus 1 GB shuffled files, window sizes log-uniform 3e-4…0.7, 10 query templates, 5 network profiles, 28 cold / 72 warm. The estimator and constants were frozen at commit `7ceaa1a`; the harness fix is the only change after it (`7ee47cc`). Calibration `calibration-v3.jsonl` (365 observations), `--no-learn`. Honest caveat on freshness: the `_s3` data files were already read by the invalid v4 run (not by any calibration); windows, queries and networks are new.

## 6. Error Distribution

See section 2 and `summary-planner-hardening-ii.md`. By quality label (output rows): *good* (n = 63) P90 1.0×, max 1.13×; *fair* (n = 37) P90 2.0×, max 2.0×; no case labelled *poor* (the label is meaningful, and conservative).

## 7. Planner Accuracy

| | held-out v5 |
|---|---|
| accuracy | **97 %** (excluding the 1 near tie: 98 %) |
| always-server baseline | 90 % (P90 regret 1.04) |
| always-browser baseline | median regret 11×, P90 119× |
| by network | FAST_LAN, GOOD_BROADBAND, MOBILE, HIGH_LATENCY: 100 %; LOCAL: 89 % (28 cases) |
| browser fastest / chosen | 10 / 7 |
| by confidence | 0.80–1.00: 99 % (90 cases); 0.65–0.80: 80 % (10 cases) |

The 3 wrong choices are all LOCAL, warm, small queries where the server took 78–83 ms and the browser 61–80 ms.

## 8. Regret

Median 1.00, **P90 1.00**, P95 1.00, max 1.28 (case h3008, 30 MB uniform, count_by). The wrong choices cost at most 28 % of a ~80 ms query.

## 9. Real Network Validation

Chrome DevTools throttling (latency, download and upload throughput) applied to the page, its workers and DuckDB-Wasm; the gateway's own simulation is not used. Time model = planner score vs measured end-to-end time:

| engine | median | P90 | P95 | max | gate (median ≤ 1.25×, P90 ≤ 3×) |
|---|---|---|---|---|---|
| server (n = 100) | 1.11× | 1.50× | 1.63× | 2.29× | **met** |
| browser (n = 98) | **1.37×** | 2.68× | 3.53× | 5.06× | median **not met**, P90 met overall |

By profile (browser median / P90): FAST_LAN 1.38 / 1.59 · GOOD_BROADBAND 1.22 / 1.88 · MOBILE 1.06 / 2.17 · **HIGH_LATENCY 1.36 / 3.43** · **LOCAL 1.71 / 3.53**. Server model by profile: all medians 1.07–1.16 except MOBILE 1.44 (P90 1.65). Limitation: throttling on loopback is a good model of latency and bandwidth but not of packet loss, jitter or a real remote server; a second machine on a real link was not available.

## 10. Browser Memory Envelope

Measured on Chrome 155 and Edge 155 with range access, 100 MB → ~1 GB (`envelope-chrome.json`, `envelope-msedge.json`): no tab crash up to about 976 MB fetched. The limiting factor is the *result*: 100 000 rows took 10–34 s and about 430 MB of JS heap (Overture subset). Operating envelope (8 GB-class device): safe fetch 300 MB, unsafe fetch 1200 MB, safe result 150 MB, unsafe result 1000 MB. These are operating limits, not physical thresholds. Above ~1 GB not tested. Scaling by `deviceMemory/8` for smaller devices is an assumption.

Safety classes: **Safe** (inside the envelope) / **Risky** (needs ≥ 30 % cost advantage) / **Unsafe** (never run automatically; 2 v5 cases were skipped for this reason) / **Unknown** (unprobed runtime or another browser family; never chosen on cost alone).

## 11. Second Browser / Device

Edge 155 was run through the same smoke, explorer, engines, planner suites (31 passed on both browsers) and the envelope runs. **It is the same Chromium engine on the same machine**, so this shows that the code is not Chrome-branded, not that it works on an independent browser engine or device. A real second device was not available. DeviceProfile (memory class, JS heap limit, engine/runtime versions, probe timing) is collected only by the probe and only used by the planner; no fingerprinting.

## 12. Runtime Probe

17 checks (worker, init, arrow, parquet, http_range, geometry_wkb, covering, two read classes, one per operator, device profile). Probes run the engine's real execution path on tiny fixtures. Held-out: 0 probe-related failures. ADR-009 is **Accepted for the measured Chromium family on one machine**.

## 13. Canonical Lowering RFC

RFC-004 (rewritten): direction **B** (the gateway ships the browser-dialect SQL, generated in Rust) reached through **E** (golden fixtures first). Step 1 is implemented: 8 fixtures in `crates/geo-engine-browser/tests/fixtures/lowering/`, Rust output == TypeScript output (regenerate with `GEN_LOWERING=1`). Step 2 (the browser executes SQL it receives) is designed with its migration plan and risks (SQL injection surface moves to the gateway, versioning of dialects, offline mode). No IR or wire-format change under any step. Status stays Proposed.

## 14. UI Productization

### Explorer
Default mode, Vietnamese business wording, summary-first execution panel ("Xử lý trên máy chủ", time, results, data processed), advanced detail behind "Chi tiết nâng cao", pipeline strip (data source → window → fields → limit → where it ran), no credits, friendly dataset names, "Lịch sử xử lý" in the inspector, inspector collapsed when nothing is selected, bottom panel 240–300 px and resizable.

### Developer
Raw field names, engine / network / probe line, Profile tab, plan ids, all planner reasons.

### Vietnamese and English
`vi`/`en` catalogs with a test that keys and placeholders match; layout identical in both (verified by screenshot). Mode and language persist (`gf.mode`, `gf.lang`) and can be set by URL (`?mode=&lang=`).

### Accessibility
ARIA tab semantics and states (booleans fixed: they used to render as empty strings), keyboard-reachable tabs, contrast test over the palette, focus visible. Tested at 1366×768 and 1920×1080.

Known UI deviations: no "Project/Profile" buttons (there is no backend for them; building affordances with nothing behind them is the thing the project rules forbid). The benchmark data directory holds many generated files, so the dataset list shows several entries with the same friendly name ("Tòa nhà — Lớn") and some generated names are shown raw (`Clustered 100mb shuffled s3`); real data would not look like this, but it is visible in the screenshots.

## 15. UI Screenshots

`docs/ui-explorer-vi.png`, `docs/ui-explorer-en.png`, `docs/ui-developer.png` (1600×900), regenerated by `UI_SHOTS=1 npx playwright test explorer`.

## 16. Regression

| suite | result |
|---|---|
| Rust `cargo test --workspace --no-default-features` | 156 passed, 0 failed |
| dependency boundaries (`tools/arch_check.py`) | OK, 7 crates |
| TypeScript unit tests | 50 passed |
| Playwright smoke + explorer + engines + planner, Chrome | 31 passed |
| same, Edge | 31 passed |
| explorer with screenshots | 15 passed |
| server benchmarks vs the oracle (`geo-bench`) | **not re-run this round**; the server execution path was not changed, and the engines suite compares server and browser result digests |

One first combined run (screenshots enabled) showed 2 failures in the engines suite (a fallback test and the Overture test); both passed alone and in two further combined runs. I did not find a cause; the likely suspect is state shared through the one gateway (health tracker) between suites running in sequence. Flagged, not explained.

## 17. ADR / RFC Status

| | status |
|---|---|
| ADR-008 multi-engine placement | Proposed (unchanged) |
| ADR-009 runtime probing | **Accepted**, scoped to Chromium family on one machine |
| ADR-010 confidence, health, network, estimator | **Proposed** (estimator gates met, browser time-model gate not met) |
| RFC-003 planner | Proposed |
| RFC-004 canonical lowering | Proposed, direction chosen, step 1 implemented |

## 18. Remaining Risks

1. **Browser time model median 1.37× (gate 1.25×)**; LOCAL and HIGH_LATENCY P90 above 3×. Not tuned on the held-out set, as required.
2. No second real device or engine; Firefox and Safari are Unknown by policy.
3. Above ~1 GB untested; memory scaling below 8 GB is an assumption.
4. The estimator's "no context" fallback path never occurred in held-out data (all cases had a histogram).
5. Filter selectivity assumes independence between columns; it showed P90 2.0× on 8 peaked/filtered cases.
6. Wrong choices cluster on LOCAL small queries (server 80 ms vs browser 60–80 ms): low cost, but it is where the planner is least sure.
7. Cold-cache outliers on the server (100 MB shuffled 1.4–2.4 s) exist.
8. Process: two held-out runs were lost to harness defects. The harness now checks, but a held-out run is only trustworthy if someone reads the first lines of each gateway log.
9. One unexplained intermittent Playwright failure pair (section 16).

## 19. Recommendation

**KEEP HARDENING PLANNER.** What the next small round should do, in order: (1) fix the browser time model for LOCAL and HIGH_LATENCY *using calibration data, not v5*, then run a new held-out once; (2) probe a genuinely different browser engine or device; (3) investigate the intermittent test failure; (4) only then ADR-010 and PostGIS.

## Commit

`daaeddb` (message: Planner hardening II ...); the report itself is in this commit.
