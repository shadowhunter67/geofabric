import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { classifyFailure, fallbackAllowed, isDeterministic, retryAllowed } from "../src/engine/failure.ts";

test("classifier agrees with the Rust implementation on the shared fixture", () => {
  const cases = JSON.parse(readFileSync(new URL("../../crates/geo-planner/tests/fixtures/failure-classes.json", import.meta.url), "utf8")) as { message: string; class: string }[];
  assert.ok(cases.length >= 15);
  for (const c of cases) assert.equal(classifyFailure(c.message), c.class, c.message);
});

test("recovery policy: bounded", () => {
  assert.equal(fallbackAllowed("CANCELLED"), false);
  assert.equal(fallbackAllowed("TIMEOUT"), true);
  assert.equal(isDeterministic("UNSUPPORTED_FORMAT"), true);
  assert.equal(isDeterministic("TRANSIENT"), false);
  for (const c of ["CAPABILITY", "MEMORY", "TIMEOUT", "CANCELLED", "ENGINE_CRASH", "UNSUPPORTED_FORMAT", "UNSUPPORTED_OPERATION", "TRANSIENT"] as const) assert.equal(retryAllowed(c), false);
});
