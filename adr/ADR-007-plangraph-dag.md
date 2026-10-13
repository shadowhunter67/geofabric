# ADR-007 — PlanGraph (DAG) is the logical plan representation

Status: Accepted (2026-10-13)

**Decision.** IR 0.2 represents a plan as `PlanGraph { nodes: NodeId -> PlanNode, root }`; nodes reference inputs by id. The tree (IR 0.1) stays as a builder/legacy type: `Plan::new(Node)` builds a graph (equal sub-trees collapse into one node), `Plan::to_tree()` expands it, `Plan::from_json` migrates 0.1 files deterministically.

**Why.** Shared sources and sub-expressions (self joins, reused filters), content-addressed lineage/caching, stable hashing independent of id spelling, and a plan shape that a multi-engine planner can reason about.

**Evidence.** 14 DAG tests (linear, branching, shared source, shared intermediate, cycle, missing node, orphan, topological order, roundtrip, id-independent plan id, migration determinism and equivalence on 5 fixtures produced by the 0.1 implementation); engine tests run a shared-source DAG with an exact result and one CTE for the shared source; the tree and DAG forms of one plan give identical digests; validation behaviour of the 12 pre-existing IR tests is unchanged.

**Consequences / limits.** DuckDB compiles each node once as a `NOT MATERIALIZED` CTE, so a shared node is *planned* once but DuckDB still evaluates it per consumer (inlined); materialisation of expensive shared nodes is a future knob. Scan analysis treats a shared source as the union of its access paths. Multi-stage plans (splitting a graph across engines) are not implemented.
