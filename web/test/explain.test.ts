import assert from "node:assert/strict";
import { test } from "node:test";
import { pipelineSteps, summarize } from "../src/ui/explain.ts";
import type { PlanGraph } from "../src/engine/contract.ts";
import type { Candidate, RunResp } from "../src/types.ts";

const ir = {
  ir_version: "0.2", root: "c",
  nodes: {
    a: { op: "source", uri: "x", format: "geo_parquet", schema: { fields: [], geometry_columns: [] } },
    b: { op: "bbox_filter", input: "a", geometry_column: "geometry", bbox: {} },
    p: { op: "project", input: "b", columns: [] },
    c: { op: "limit", input: "p", n: 10 },
  },
} as unknown as PlanGraph;

const cand = (engine: string, kind: "server" | "browser", score: number | null, extra: Partial<Candidate> = {}): Candidate => ({
  engine, kind, feasible: score != null, score_ms: score, cost: null, reasons: [], rejected_because: [], confidence: null, risk: null,
  health: { state: "unknown", reason: "", events: 0, failures: 0, last_failure: null }, probe_status: "fresh", ...extra,
});

function run(p: Partial<RunResp["physical"]>, over: Partial<RunResp> = {}): RunResp {
  return {
    plan_id: "p", ir, lineage: [], columns: [], rows: [], rows_total: 20000, geojson: null,
    metrics: {} as RunResp["metrics"], timing: { plan_and_overhead_ms: { value: 1, source: "x" }, total_ms: { value: 92, source: "x" }, boundary_conversion_ms: { value: 1, source: "x" } },
    physical: {
      engine: "duckdb", placement: "server", decisions: [], steps: [], candidates: [cand("duckdb", "server", 90), cand("duckdb-wasm", "browser", 446)],
      scans: [{ uri: "x", total_bytes: 1000, prune: { kept: [0], skipped: 9, total_row_groups: 10, total_rows: 100, kept_rows: 10, kept_bytes: 20, total_bytes: 1000 } }],
      assessment: { policy: [], browser_access: [] } as unknown as RunResp["physical"]["assessment"], ...p,
    },
    ...over,
  };
}

test("the pipeline reads from the data source to the place of execution, without engine names", () => {
  const s = pipelineSteps(ir, "server");
  assert.deepEqual(s.map((x) => x.key), ["source", "bbox_filter", "project", "limit", "server"]);
  assert.equal(s[1].title, "pipe.bbox_filter");
  assert.equal(pipelineSteps(ir, "browser").at(-1)!.title, "pipe.browser");
  assert.deepEqual(pipelineSteps(null, "server").map((x) => x.key), ["server"]);
});

test("a plan with an unknown operator still gets a step (never hidden)", () => {
  const odd = { ir_version: "0.2", root: "a", nodes: { a: { op: "mystery", input: "b" }, b: { op: "source" } } } as unknown as PlanGraph;
  assert.equal(pipelineSteps(odd, "server").length, 3);
});

test("server chosen because it is faster; the slower alternative is reported with its estimate", () => {
  const sm = summarize(run({}));
  assert.equal(sm.where, "server");
  assert.deepEqual(sm.reasons, ["exec.why.faster"]);
  assert.deepEqual(sm.alt, { where: "browser", timeMs: 446, reason: null });
  assert.equal(sm.timeMs, 92);
  assert.equal(sm.rows, 20000);
  assert.ok(Math.abs(sm.skippedFraction! - 0.98) < 1e-9, "20 of 1000 bytes were needed");
  assert.equal(sm.memoryBytes, null);
});

test("an unavailable alternative is explained in plain words, and the server is 'the only suitable option'", () => {
  const sm = summarize(run({ candidates: [cand("duckdb", "server", 90), cand("duckdb-wasm", "browser", null, { rejected_because: ["browser workload is unsafe: could pull 3000 MB into the tab"] })] }));
  assert.deepEqual(sm.reasons, ["exec.why.only"]);
  assert.equal(sm.alt!.reason, "exec.reason.too_large");
  const un = summarize(run({ candidates: [cand("duckdb", "server", 90), cand("duckdb-wasm", "browser", null, { rejected_because: ["unsupported operations: intersects"] })] }));
  assert.equal(un.alt!.reason, "exec.reason.unsupported");
});

test("an explicit choice, a fallback and a cautious choice each say so", () => {
  assert.deepEqual(summarize(run({}), true).reasons, ["exec.why.chosen"]);
  assert.deepEqual(summarize(run({}, { fallback: { from: "duckdb-wasm", reason: "x", class: "ENGINE_CRASH" } })).reasons, ["exec.why.fallback"]);
  const cautious = summarize(run({ candidates: [cand("duckdb", "server", 120), cand("duckdb-wasm", "browser", 90)], assessment: { policy: ["R5b close call"], browser_access: [] } as never }));
  assert.deepEqual(cautious.reasons[0], "exec.why.cautious");
});

test("browser processing reports the data it pulls and the memory it needs, never an invented server number", () => {
  const sm = summarize(run({
    engine: "duckdb-wasm", placement: "browser",
    candidates: [cand("duckdb", "server", 200), cand("duckdb-wasm", "browser", 60, { risk: { memory_input_bytes: 9_300_000, safety: "safe" } as never })],
    assessment: { policy: [], browser_access: [{ fetch_bytes_est: 9_300_000 }] } as never,
  }));
  assert.equal(sm.where, "browser");
  assert.equal(sm.memoryBytes, 9_300_000);
  assert.equal(sm.processBytes, 9_300_000);
  assert.ok(sm.reasons.includes("exec.why.no_server"));
});
