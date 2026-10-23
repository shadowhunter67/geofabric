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
