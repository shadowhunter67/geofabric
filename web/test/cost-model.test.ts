import assert from "node:assert/strict";
import { test } from "node:test";
import { semiJoinLine, summarizeCostModel, uncertaintyLines } from "../src/ui/cost-model.ts";
import type { CostModelVerdict, SemiJoinInfo } from "../src/types.ts";

const base: CostModelVerdict = {
  model_version: "postgis-cost-v1.1", workload_class: "indexed_bbox", validation: "validated", time_ms: 72.4,
  terms: [{ name: "startup", ms: 16.4 }, { name: "scan", ms: 0.2 }, { name: "result", ms: 40 }, { name: "result_geometry", ms: 15.8 }],
  risk: { blocked: false, level: "low", reason: "" }, index_state: "available", notes: [],
};

test("a validated, unblocked, low-risk class is ok and lists its biggest cost terms", () => {
  const s = summarizeCostModel(base);
  assert.equal(s.level, "ok");
  assert.equal(s.predicted_ms, 72.4);
  assert.equal(s.top_terms, "result 40 ms, startup 16 ms, result_geometry 16 ms", "terms under 0.5 ms are dropped, the rest sorted by size");
  assert.equal(s.risk, "low");
  assert.equal(s.timeout, "", "an engine with no timeout opinion shows none");
});

test("a class that is not validated is a warning, whatever its predicted time", () => {
  for (const validation of ["limited", "unvalidated"] as const) assert.equal(summarizeCostModel({ ...base, validation }).level, "warn");
});

test("a refused workload says so, says whether the block can be lifted, and is never ok", () => {
  const s = summarizeCostModel({ ...base, workload_class: "semijoin_metric_large", validation: "limited", risk: { blocked: true, overridable: true, level: "high", reason: "repeated-probe risk" } });
  assert.equal(s.level, "warn");
  assert.equal(s.risk, "high (refused, liftable)");
  assert.equal(s.risk_reason, "repeated-probe risk");
});

test("the timeout probability is shown with sensible precision, and unknown is shown as unknown (never as 0)", () => {
  assert.equal(summarizeCostModel({ ...base, risk: { blocked: false, level: "timeout_risk", reason: "", timeout_probability: 0.874 } }).timeout, "87 %");
  assert.equal(summarizeCostModel({ ...base, risk: { blocked: false, level: "low", reason: "", timeout_probability: 0.0123 } }).timeout, "1.2 %");
  assert.equal(summarizeCostModel({ ...base, risk: { blocked: false, level: "unknown", reason: "feature missing", timeout_probability: null } }).timeout, "unknown");
  assert.equal(summarizeCostModel({ ...base, risk: { blocked: false, level: "timeout_risk", reason: "" , timeout_probability: 0.9 } }).level, "warn");
});

test("an elevated risk or an index state that is not available is visible even for a validated class", () => {
  assert.equal(summarizeCostModel({ ...base, risk: { blocked: false, level: "elevated", reason: "x" } }).level, "warn");
  assert.equal(summarizeCostModel({ ...base, index_state: "unknown", risk: { blocked: false, level: "elevated", reason: "index state 'unknown'" } }).index_state, "unknown");
});

const sj: SemiJoinInfo = { expected_probes: 252681, right_rows: 28786, metric: true, predicate: "within_distance", distance_m: 500, right_rows_filtered: 28786, candidates_per_probe: 108.8, candidate_pairs: 27498398, overlap_ratio: 0.93, estimate_basis: "histogram" };

test("the semi-join line lists the candidate-work features and names the unknown ones instead of printing zeros", () => {
  const l = semiJoinLine(sj)!;
  assert.match(l.text, /candidate pairs 27,498,398/);
  assert.match(l.text, /distance 500 m/);
  assert.deepEqual(l.unknownFeatures, []);
  const u = semiJoinLine({ ...sj, candidate_pairs: null, right_rows_filtered: null })!;
  assert.deepEqual(u.unknownFeatures, ["right_rows_filtered", "candidate_pairs"]);
  assert.match(u.text, /candidate pairs unknown/);
  assert.equal(semiJoinLine({ ...sj, metric: false }), null, "an intersects semi join has no metric candidate work");
  assert.equal(semiJoinLine(undefined), null);
});

test("uncertainty lines carry the policy version, and are empty when the planner is not uncertain", () => {
  assert.deepEqual(uncertaintyLines(null), []);
  assert.deepEqual(uncertaintyLines({ state: "uncertain", policy_version: "metric-semijoin-policy-v1", reasons: [] }), []);
  assert.deepEqual(uncertaintyLines({ state: "uncertain", policy_version: "metric-semijoin-policy-v1", reasons: ["no engine is known to be safe"] }), ["no engine is known to be safe (metric-semijoin-policy-v1)"]);
});
