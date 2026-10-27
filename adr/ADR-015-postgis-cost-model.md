# ADR-015: PostGIS cost model, semi-join risk policy and class-scoped R5d

Status: **Accepted (2026-10-23)** for the scope below. Supersedes the blanket form of policy R5d in ADR-013 ("an unmeasured engine is never chosen on predicted cost") with a **class-scoped** form. No IR change; no new engine; the public static demo is unchanged (it never contacts PostGIS).

## Context

ADR-013 added PostGIS as a peer engine and proved it correct (17/17 plans identical to DuckDB, Demo B = the independent oracle). It could not say when PostGIS is the *right* engine: its cost was a stand-in copy of DuckDB's formula, so policy R5d kept the planner on DuckDB always. Measurements showed that this costs a lot where PostGIS is right (an admin semi-join: DuckDB 4-60 s, PostGIS 0.05-0.3 s; an indexed window: 150-450 ms vs 16-60 ms) and that no scalar multiplier can model PostGIS (it loses large metric semi-joins by 5-10x).

## Decision

1. **Cardinality and engine cost stay separate.** The planner's estimator keeps producing the row estimate; from it and the plan it derives `WorkloadFeatures` (plan-time facts only: matching rows, result rows/bytes, operation flags, limit, semi-join probes / right rows / metric / distance). An engine may implement `EngineDescriptor::cost_estimate(plan, features)` and return a model verdict: model version, **workload class**, **validation level** (`validated | limited | unvalidated`), time and its terms, a **hard risk verdict**, and the index state the formula assumed. The planner learns nothing about what is inside the model.
2. **Workload classes come from features, never from names** (`geo_executor::cost_model::workload_class`): `indexed_bbox`, `indexed_spatial` (Intersects / projected column), `aggregate`, `broad_scan` (> 300,000 rows that must be read; an *unordered* LIMIT stops early and does not count), `semijoin_intersects`, `semijoin_metric_small` (<= 5,000 right-side probes), `semijoin_metric_large`. The two thresholds were read off the calibration set and fixed before any held-out run.
3. **Transparent formulas.** Per class a non-negative sum of named terms (`startup`, `scan`, `result`, `result_geometry`, `probe`, `right`), fitted by NNLS on relative error, stored with provenance in `crates/geo-engine-postgis/src/cost_model_v1.json` (commit, seeds, dataset versions, machine class, build profile, method). DuckDB keeps its measured shared formula and gains the same kind of class model **only** for the semi-join shapes (its shared formula was x26 off on the admin join), file `duckdb-semijoin-cost-v1.1`.
4. **R5d is scoped by validated class.** A candidate counts as *measured* iff its engine's model is `validated` for the plan's class **and** the index state is `available` (derived from the catalog: `spatially_indexed`, `metric_indexes`; `unknown` is never read as `available`). Otherwise the engine keeps its engine-level calibration (PostGIS: unmeasured, R5d applies; DuckDB: measured). An explicit engine hint still selects any feasible engine.
5. **The semi-join risk policy is a hard rule, not a coefficient.** A metric semi-join with more than 50,000 right-side probes is refused for *automatic* selection of PostGIS (`risk.blocked` makes the candidate infeasible; an explicit request overrides it and says so). Between 5,000 and 50,000 the risk is `elevated`. No dataset name is involved.
6. **Validation is evidence, gated in advance.** `benchmarks/postgis_cost_gates.json` was committed before the held-out run: a class is auto-selectable only with >= 15 held-out cases, P90 regret <= 1.5, P90 / median prediction ratio <= 3.0 / 1.6, 0 unsafe selections, 0 correctness mismatches and an available index. Levels live in `cost_validation_v1.json`.

## Evidence (all on one machine: i7-14700, 32 GB, Windows 11, loopback PostgreSQL 16.9 + PostGIS 3.6.2, DuckDB 1.5.6, release build, 3.85 M buildings in PostGIS)

Corpus: 240 calibration cases (seed 20261103), held-out A 220 (20261107), confirmation B 260 (20261111, enriched in semi-joins); engine order alternated per case; every comparable result identical cell by cell and coordinate by coordinate (0 mismatches in 720 cases). Model v1 (fit on calibration) was assessed on A; v1.1 (refit on calibration + A) was assessed on B with the coefficients shipped. A decided the initial eligibility (3 classes); B confirmed those and promoted `semijoin_intersects` (it failed only the median-prediction gate on A: 1.78 vs 1.6, a bias the refit removed: 1.16 on B). Timeouts (DuckDB, 60 s) enter as censored observations.

| Class | PostGIS model | Auto-select | Evidence (B, 260 cases) |
|---|---|---|---|
| `indexed_bbox` | validated | **yes** | n=73, P90 regret 1.00, prediction median 1.03 / P90 1.66 |
| `indexed_spatial` | validated | **yes** | n=22, P90 1.00 (max 1.86, 323 ms), prediction 1.12 / 2.95 |
| `aggregate` | validated | **yes** | n=47, P90 1.00, prediction 1.13 / 2.32 |
| `semijoin_intersects` (admin) | validated | **yes** | n=25, regret 1.00 in all (DuckDB 12 timeouts at 60 s), prediction 1.16 / 1.85 |
| `broad_scan` | limited | no | n=19, P90 regret 2.41, prediction P90 3.66 > 3.0 |
| `semijoin_metric_small` | limited | no | n=32, regret passes when PostGIS is chosen (P90 1.32) but prediction median 2.85 / P90 5.09 fail the numeric gates |
| `semijoin_metric_large` | limited (+ hard block > 50k probes) | no | n=39, P90 regret 3.6, prediction 2.33 / 6.84; the faster engine flips with right-side density and distance |

Not validated for: large metric semi-joins, unknown index state, remote PostgreSQL (latency was not calibrated), another machine. Plan-time features cannot predict the metric semi-join time better than a median x2 (a density-aware term was tried on the calibration data and did not help).

## Consequences

* Inside the four validated classes the planner has P90 regret 1.00 (B: n=167; A: n=156) against 14.7 (B) / 12.1 (A) for the old always-DuckDB behaviour; the mean absolute penalty on B fell from 3.6 s to 0.2 s.
* Outside them R5d still applies, so a few expensive choices remain: on A two huge unfiltered semi-joins (250k and 530k probes) go to DuckDB, which timed out at 60 s while PostGIS finished in 12-18 s. That is the old behaviour and the regime where neither engine is reliably safe; the explanation carries a DuckDB warning above 200,000 probes. It is reported as unsafe in the *R5d path*, not as an auto-selection.
* `SharedPostGis` (the runtime's wrapper) must forward `cost_estimate`; a runtime test pins it (it was missing on the first run and hid the model from the planner).
* The model is machine-scoped: a different host needs a recalibration run (`benchmarks/make_postgis_cost_cases.py`, `crates/geo-runtime/tests/postgis_cost.rs`, `postgis_cost_fit.py`, `postgis_cost_eval.py`), and the validation file must be regenerated from its own held-out set.

## Not done (P1)

A two-stage bbox prefilter or generated projected column for the metric semi-join (would change the physical design); promotion of `semijoin_metric_small` (needs a better numeric fit or a documented gate amendment confirmed on a fresh set); a second physical machine; a remote database with real latency; TLS; PostGIS-only source planning (the planner still reads Parquet footers; the PostGIS relation lacks nested columns, so `near` returning every column is not PostGIS-eligible).

## Addendum (2026-10-26): connector latency bug, remeasurement, model v2 — supersedes the class table above

**What changed.** While investigating an unexplained process exit I found that the PostGIS connector's cancel-watcher slept in 15 ms steps and the query path `join()`ed it, so **every PostGIS query was padded by up to one Windows timer tick (~15 ms)**: 55 % of all measured PostGIS times sat on 16 ms. Fixed (the watcher now waits on a channel and wakes at once). After the fix indexed queries take **1-3 ms** (0.8 % of times at 16 ms). Every PostGIS timing and every coefficient above was therefore wrong in its fast range, so the three sets were re-measured (720 cases, 0 correctness mismatches), a model **`postgis-cost-v2`** was refit (objective: mean squared log(pred/actual), the gates' own metric), and validation was **reset to empty** and re-earned on sets that did not exist at fit time.

**The unexplained exit was not an engine crash.** Both aborts (exit `0xffffffff`) coincide with entries in the machine's RAM emergency-killer log ("killed postgis_cost-... (2.1 GB)" 2026-10-23 14:08:43 and 2026-10-26 11:20:59). A separate 44-case DuckDB stress run at the 60 s boundary (7 timeouts, one process) did not crash. One real defect was fixed on that path (an interrupted statement was executed a second time).

**Evidence (all fresh data, 0 mismatches):** fresh set seed 20261115 (300 cases): under the gates as committed, only `semijoin_intersects` validated; `indexed_bbox` and `aggregate` had P90 regret 1.00 but P90 prediction ratio 3.29 / 3.12 (> 3.0). That ratio, with times of 1-3 ms floored at 1 ms, measures timer noise: DuckDB was never faster than 64 ms in these classes, so no decision turns on it. **Gate amendment, made after seeing that result** (`postgis_cost_gates.json`, "amendments"): both times are floored at 10 ms for the prediction ratio; everything else is unchanged, the amendment was committed before a new independent set (seed 20261119, 300 cases) was generated, and the fresh set is not allowed to promote anything under it. On 20261119 aggregate, broad_scan, indexed_bbox, indexed_spatial and semijoin_intersects passed; metric_small (prediction P90 3.41, median 1.68) and metric_large (P90 regret 4.4, one unsafe selection) did not. Because the two independent sets disagreed on two classes (indexed_spatial P90 regret 1.59 and broad_scan 2.09 on 20261115), **a class is validated only if it passes on both**.

| Class | PostGIS model | Auto-select | Evidence (20261119 / 20261115, amended gates) |
|---|---|---|---|
| `indexed_bbox` | validated | **yes** | n=81 / 82, regret P90 1.00 / 1.00, prediction P90 1.49 / 2.25 |
| `aggregate` | validated | **yes** | n=61 / 61, regret P90 1.28 / 1.00, prediction P90 2.74 / 2.77 |
| `semijoin_intersects` | validated | **yes** | n=29 / 29, regret 1.00 / 1.00, prediction P90 1.99 / 2.37 |
| `indexed_spatial` | limited | no | passed 20261119 (n=22), failed 20261115 (regret P90 1.59) |
| `broad_scan` | limited | no | passed 20261119 (n=18), failed 20261115 (regret P90 2.09) |
| `semijoin_metric_small` | limited | no | prediction P90 3.41 / median 1.68 on 20261119 |
| `semijoin_metric_large` | limited (+ block > 50k probes) | no | regret P90 4.4 / 2.6, one unsafe selection per set |

**Planner results with this validation** (replayed on the stored cases): inside the validated scope (n=171 / 172) P90 regret **1.00**, max 4.28 / 4.91, **max absolute penalty 134 ms / 376 ms**. Over *all* cases P90 regret is 6.99 / 3.11 and the primary gate "P90 <= 1.5" is **not met**: with PostGIS answering in 1-3 ms, every case that R5d leaves on DuckDB (limited classes, ~40 % of these enriched sets) shows a large *ratio* (DuckDB 70-100 ms vs 2 ms) and the metric semi-joins show multi-second penalties (max 53 s). Mean absolute penalty is 396 / 473 ms against 2.5 / 3.1 s for always-DuckDB, but **always-PostGIS (154 / 260 ms) is better on this corpus** for the reason above; it is not a shippable policy (unsupported plans, clipped relation, the risk block). One unsafe selection per set, both from the R5d/DuckDB path (a huge metric semi-join on which DuckDB hit its 60 s timeout while PostGIS finished).

**Open P1s changed:** the DuckDB-abort P1 is closed (cause: the RAM killer); a scale warning above 250,000 probes now exists in the planner's decisions; a second machine / remote PostgreSQL was NOT possible (no second device or remote server available) and remains P1.

## Addendum 2 (2026-10-26/27): metric semi-join safety, uncertainty policy `metric-semijoin-policy-v1`

**Problem.** Two held-out cases: R5d kept DuckDB because PostGIS was unvalidated for the class, DuckDB hit its 60 s timeout, PostGIS finished in 12-18 s. "PostGIS is unvalidated" had been read as "DuckDB is safe".

**Decision.**
1. **Cost, validation and risk are separate for every engine.** An engine's model reports a risk level (`low | elevated | timeout_risk | unknown | high`), a timeout probability, and whether a block is cost-based and `overridable`.
2. **R5e** (RFC-003): if the selected engine is at `timeout_risk`, the planner chooses an engine that is not (lifting a cost-based block for that reason alone); if every option is at risk it takes the least likely to time out and attaches `Assessment.uncertainty`; `unknown` risk is reported as uncertainty and never read as low. Explicit engine requests bypass R5e. No speculative dual execution, no extra retries or fallbacks.
3. **Plan-time candidate-work features** (histogram based; `null` = unknown, never zero): right side after its attribute filter, density, candidates per probe, **candidate pairs**, overlap. They exist because the footer-only estimates the earlier models were fitted on were far off (left rows P90 x5.3, right x13; with histograms x1.3-1.8 and x1.6-4.6). The benchmark harness now builds the histograms (`Runtime::ensure_histogram`, shared with the gateway), so it measures the planner that ships.
4. **Classes** by plan-time features: `semijoin_metric_small` (<= 5,000 probes), `semijoin_metric_medium` (<= 50,000 probes and < 1e7 candidate pairs), `semijoin_metric_large` (the rest, and any plan whose pairs are unknown). The 1e7 threshold is where DuckDB left the 1-30 s range: all 26 observed timeouts have >= 7.8e7 pairs.
5. **Risk models** (separate versions): `duckdb-timeout-risk-v1` and `postgis-timeout-risk-v1`, log-linear Tobit models (timeouts censored) fitted only on the regime where timeouts occur (>= 1e7 candidate pairs for DuckDB, >= 50,000 probes for PostGIS); P >= 0.10 `elevated`, P >= 0.20 `timeout_risk`. PostGIS' repeated-probe block (> 50,000 probes) stays, as an overridable cost-based block.
6. **Cost models** `postgis-cost-v3`, `duckdb-semijoin-cost-v3`: pooled log-linear for the three metric classes; every other class keeps its v2 coefficients and evidence (the validated classes were not refitted).

**Evidence (all stages separated).** Discovery: 480 metric semi joins (earlier sets re-planned with histograms + 150 new, seed 20261201). Calibration: 200 new (20261205). Everything frozen at commit `4a174f0`. Confirmation: 320 balanced cases (20261209) and 70 heavy-regime cases (20261213, generated after the balanced set showed only 10 heavy cases), 0 correctness mismatches in all. Gates unchanged (`postgis_cost_gates.json`).

| Result | Value |
|---|---|
| Heavy set (70): DuckDB timeouts | 16 of 70; all 16 rated `timeout_risk` in advance, PostGIS chosen, 0 timeouts; previous policy: 18 timeouts, 41 plans on a `timeout_risk` engine |
| Heavy set: the one miss | a DuckDB run that completed at 61.0 s (P = 0.14, below the 0.20 threshold) |
| False alarms | 7 plans rated `timeout_risk` where DuckDB completed; PostGIS was 4-15x faster in them, so no loss |
| Balanced set (320): metric timeouts | none; 3 plans rated `timeout_risk` (P 0.44-0.47), PostGIS chosen 4-15x faster. The 8 DuckDB timeouts of that set were all admin semi joins, already routed to PostGIS by the validated class |
| Fixed regression | all 10 earlier DuckDB timeouts are rated `timeout_risk` by the shipped model (CI test); both earlier "unsafe" held-out cases now plan on PostGIS (11.3-11.7 s) |

**Promotion experiments** (plan as if the class were validated; fresh balanced set; gates as committed): `semijoin_metric_small` regret P90 1.00 but prediction P90 x3.48 / median x1.70 (gate 3.0 / 1.6) - not promoted; `semijoin_metric_medium` regret P90 1.77 - not promoted; `semijoin_metric_large` regret P90 1.61 - not promoted. Validated scope is unchanged: `indexed_bbox`, `aggregate`, `semijoin_intersects`.

**Not achieved.** The global P90 regret gate (<= 1.5): balanced set 8.23, heavy set 4.42. The ratio is dominated by limited classes where PostGIS answers in 1-5 ms and DuckDB in 70-600 ms (absolute penalty <= 0.6 s there); the multi-second penalties are metric semi joins. Inside the validated scope: P90 1.00, max 3.0, max penalty 81 ms.

**Consequences / not done.** Cardinality estimation is no longer the main source of model error (with TRUE counts PostGIS' error P90 falls x3.09 -> x2.45, DuckDB's does not change: its variance is intrinsic); DuckDB's expected time in the heavy regime is only good to P90 x9, which is why the *risk* model, not the cost model, carries the safety. A second machine, a remote PostgreSQL and a physical-design change for PostGIS remain open (P1).
