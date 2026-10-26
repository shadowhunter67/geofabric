import assert from "node:assert/strict";
import { test } from "node:test";
import { checkGatewayIdentity } from "../e2e/gateway-identity.ts";

const HEAD = "a".repeat(40);
const OTHER = "b".repeat(40);

test("a clean build at HEAD with a fresh binary is accepted", () => {
  const v = checkGatewayIdentity({ build: { commit: HEAD, dirty: false }, head: HEAD, binaryMtimeMs: 10_000, newestSourceMtimeMs: 9_000 });
  assert.equal(v.ok, true);
  assert.deepEqual(v.problems, []);
});

test("a clean build from another commit is stale", () => {
  const v = checkGatewayIdentity({ build: { commit: OTHER, dirty: false }, head: HEAD });
  assert.equal(v.ok, false);
  assert.match(v.problems[0], /built from bbbbbbbbbb but HEAD is aaaaaaaaaa/);
});

test("a binary older than the newest Rust source is stale even when the commit matches (the locked-executable case)", () => {
  const v = checkGatewayIdentity({ build: { commit: HEAD, dirty: false }, head: HEAD, binaryMtimeMs: 5_000, newestSourceMtimeMs: 60_000 });
  assert.equal(v.ok, false);
  assert.match(v.problems.join(" "), /older than the newest Rust source/);
});

test("an old binary that reports no build identity is refused", () => {
  const v = checkGatewayIdentity({ build: undefined, head: HEAD });
  assert.equal(v.ok, false);
  assert.match(v.problems[0], /does not report a build identity/);
});

test("a dirty build at HEAD with nothing newer is accepted but noted", () => {
  const v = checkGatewayIdentity({ build: { commit: HEAD, dirty: true }, head: HEAD, binaryMtimeMs: 10_000, newestSourceMtimeMs: 9_000 });
  assert.equal(v.ok, true);
  assert.match(v.notes[0], /uncommitted Rust changes/);
});
