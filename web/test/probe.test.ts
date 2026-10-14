import { test } from "node:test";
import assert from "node:assert/strict";
import { isFresh, memoryClassFor, PROBE_TTL_MS, usable } from "../src/engine/probe-model.ts";
import type { ProbeReport } from "../src/engine/probe-model.ts";

const report = (over: Partial<ProbeReport> = {}): ProbeReport => ({
  engine: "duckdb-wasm", session: "s", engine_version: "1", runtime_version: "v", captured_at_ms: 1_000, duration_ms: 5,
  checks: [{ name: "init", ok: true, required: true, detail: "", ms: 1 }, { name: "read_class:duckdb_typed", ok: false, required: false, detail: "x", ms: 1 }],
  memory_class: "high", device_memory_gb: 8, js_heap_limit_mb: 4096, supported_ops: ["source"], ...over,
});

test("memory classes use the same thresholds as the Rust planner", () => {
  assert.equal(memoryClassFor(null), "unknown");
  assert.equal(memoryClassFor(1), "low");
  assert.equal(memoryClassFor(2), "low");
  assert.equal(memoryClassFor(4), "standard");
  assert.equal(memoryClassFor(8), "high");
});

test("a failed optional check does not make the runtime unusable; a failed required one does", () => {
  assert.equal(usable(report()), true);
  assert.equal(usable(report({ checks: [{ name: "init", ok: false, required: true, detail: "boom", ms: 1 }] })), false);
  assert.equal(usable(report({ checks: [] })), false, "no checks = nothing was proven");
});

test("freshness follows the TTL (and a missing probe is not fresh)", () => {
  const r = report();
  assert.equal(isFresh(r, 1_000 + PROBE_TTL_MS), true);
  assert.equal(isFresh(r, 1_000 + PROBE_TTL_MS + 1), false);
  assert.equal(isFresh(null), false);
});
