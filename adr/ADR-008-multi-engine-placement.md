# ADR-008 — Multi-engine placement: descriptor + executor, calibrated cost model

Status: Proposed (2026-10-13)

**Decision.** Engines are described to the planner by `EngineDescriptor` (capabilities incl. `kind` Server|Browser, limits, estimate, explain); in-process engines also implement `ExecutionEngine::execute`. The planner scores feasible candidates with a linear cost model fitted on measurements and returns every candidate with its score and reasons. The browser engine (DuckDB-Wasm) is executed by a TypeScript implementation of the same contract; the runtime returns `Dispatch::Client` when it is selected.

**Evidence.** Cross-engine digest equality (features, aggregate, filter) and the crossover benchmark (48 cases, 4 datasets incl. real Overture); planner winner agreement 43/49 on the fitting data (in-sample); engine-selection matrix tests on real files with real descriptors.

**Why it stays Proposed.** (1) Both engines are DuckDB (different builds): replaceability across *different* engine families is still unproven. (2) The cost model is fitted on one machine with loopback network; the network term is an assumption. (3) Measured browser failure modes (tab crash after many large files in one instance; DuckDB-Wasm cannot read natively-typed Parquet GEOMETRY columns such as the Overture subset) are handled by recycling and by fallback to the server, not by the planner knowing in advance. (4) Multi-stage plans are not implemented.
