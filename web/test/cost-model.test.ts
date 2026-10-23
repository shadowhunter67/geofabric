import assert from "node:assert/strict";
import { test } from "node:test";
import { summarizeCostModel } from "../src/ui/cost-model.ts";
import type { CostModelVerdict } from "../src/types.ts";

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
});

test("a class that is not validated is a warning, whatever its predicted time", () => {
  for (const validation of ["limited", "unvalidated"] as const) assert.equal(summarizeCostModel({ ...base, validation }).level, "warn");
});

test("a refused workload says so, with the policy's reason, and is never ok", () => {
  const s = summarizeCostModel({ ...base, workload_class: "semijoin_metric_large", validation: "limited", risk: { blocked: true, level: "high", reason: "repeated-probe risk" } });
  assert.equal(s.level, "warn");
  assert.equal(s.risk, "high (refused)");
  assert.equal(s.risk_reason, "repeated-probe risk");
});

test("an elevated risk or an index state that is not available is visible even for a validated class", () => {
  assert.equal(summarizeCostModel({ ...base, risk: { blocked: false, level: "elevated", reason: "x" } }).level, "warn");
  assert.equal(summarizeCostModel({ ...base, index_state: "unknown", risk: { blocked: false, level: "elevated", reason: "index state 'unknown'" } }).index_state, "unknown");
});
