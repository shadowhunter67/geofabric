# ADR-008 — Multi-engine placement: descriptor + executor, calibrated cost model

Status: Accepted, scoped (2026-10-19; was Proposed since 2026-10-13). Scope limits are part of the decision — see "Acceptance (2026-10-19)" at the end.

**Decision.** Engines are described to the planner by `EngineDescriptor` (capabilities incl. `kind` Server|Browser, limits, estimate, explain); in-process engines also implement `ExecutionEngine::execute`. The planner scores feasible candidates with a linear cost model fitted on measurements and returns every candidate with its score and reasons. The browser engine (DuckDB-Wasm) is executed by a TypeScript implementation of the same contract; the runtime returns `Dispatch::Client` when it is selected.

**Evidence.** Cross-engine digest equality (features, aggregate, filter) and the crossover benchmark (48 cases, 4 datasets incl. real Overture); planner winner agreement 43/49 on the fitting data (in-sample); engine-selection matrix tests on real files with real descriptors.

**Why it stays Proposed.** (1) Both engines are DuckDB (different builds): replaceability across *different* engine families is still unproven. (2) The cost model is fitted on one machine with loopback network; the network term is an assumption. (3) Measured browser failure modes (tab crash after many large files in one instance; DuckDB-Wasm cannot read natively-typed Parquet GEOMETRY columns such as the Overture subset) are handled by recycling and by fallback to the server, not by the planner knowing in advance. (4) Multi-stage plans are not implemented.

**Correction (2026-10-14, planner hardening — see ADR-009).** Two statements above were wrong. (a) "DuckDB-Wasm cannot read natively-typed Parquet GEOMETRY columns" — it reads them; our TypeScript lowering streamed a `GEOMETRY('OGC:CRS84')` column, which DuckDB-Wasm cannot do (fixed with `ST_AsWKB` at the scan). (b) The browser measurements behind the cost model were taken while DuckDB-Wasm downloaded **whole files** (no `Range` header; found with a request log) — "no struct-statistics pruning" and the 245 MB "crash threshold" were artefacts of that. The engine now uses real range requests (strategy `full` ≤ 32 MB, `range` above, decided by the planner); the browser cost terms were refitted on 144 runs of the calibration grid and checked on a held-out set. Status remains Proposed: both engines are still DuckDB, the network is simulated, one machine.


## Acceptance (2026-10-19) — scoped

Supported by evidence (REPORT-GEOFABRIC-FINAL-PLANNER-VALIDATION.md): a fresh held-out of 200 cases (seed 20261026) with the model, policy and estimator frozen beforehand: planner accuracy 95 % (always-server 81.5 %), regret P90 1.00 (max 1.59, 75 ms), 0 unsafe choices, 0 crashes, cross-engine digests equal in every checked case; confidence, health and fallback mechanics covered by unit and end-to-end tests (injected browser failure → one server fallback); network model validated under Chrome-level throttling for five profiles; large-file behaviour decided by policy R5c and validated on 45 cases above 128 MiB.

**Scope limits (binding):**
1. Placement between ONE server engine (DuckDB) and ONE browser engine (DuckDB-Wasm), single-source plans. Multi-stage plans and replaceability across *different engine families* (PostGIS) are NOT covered; the second point must be proven by that phase, not assumed from this one. **Update 2026-10-20 (ADR-013):** a PostGIS adapter now exists under the same contract with only two backward-compatible additions to it (`source_residency`, `calibration`); the abstraction held in tests with a mock database connection. Replaceability on a *live* PostgreSQL (data parity, cross-engine digests, measured cost) is still NOT proven.
2. Browser auto-execution is validated up to 128 MiB per source. Larger sources default to the server (R5c); an explicit engine choice still works.
3. All timing evidence comes from ONE physical machine (Chromium-family browsers; WebKit and Firefox only for result correctness). The cost model carries a measured common-mode bias on fast links (LOCAL/FAST_LAN under-predicted ~1.4–1.8×) and is not claimed to transfer to other hardware: independent device evidence is still required for ADR-010.
4. Network profiles are emulated (CDP), not measured on a real network.
