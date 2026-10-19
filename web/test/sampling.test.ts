import { test } from "node:test";
import assert from "node:assert/strict";
import { RENDER_CAP, sampleIfCapped, sampleModulus } from "../src/sampling.ts";

test("modulus is the smallest power of two that brings the count under the budget", () => {
  assert.equal(sampleModulus(100, 20000), 1);
  assert.equal(sampleModulus(20000, 20000), 1);
  assert.equal(sampleModulus(20001, 20000), 2);
  assert.equal(sampleModulus(87342, 20000), 8); // 87,342 / 4 = 21,835 > 20,000; / 8 = 10,918
  assert.equal(sampleModulus(1_000_000, 20000), 64);
  assert.equal(sampleModulus(10, 0), 1, "a bad budget never divides by zero");
  for (const total of [30000, 87342, 250000, 5_000_000]) {
    const m = sampleModulus(total, RENDER_CAP);
    assert.ok(Number.isInteger(Math.log2(m)));
    assert.ok(total / m <= RENDER_CAP && (m === 1 || total / (m / 2) > RENDER_CAP), `${total} -> ${m}`);
  }
});

test("the same count always gives the same modulus (stable while panning)", () => {
  assert.equal(sampleModulus(87342, 20000), sampleModulus(87342, 20000));
  // a pan that changes the count a little does not change the sample unless a power of two is crossed
  assert.equal(sampleModulus(81000, 20000), sampleModulus(87342, 20000));
});

test("an answer under the budget is kept as it is and the count is never asked", async () => {
  let asked = 0;
  const r = await sampleIfCapped({ rows_total: 500 }, 20000, async () => { asked++; return 500; }, async () => ({ rows_total: 1 }));
  assert.deepEqual(r, { result: { rows_total: 500 }, sampling: null });
  assert.equal(asked, 0);
});

test("a capped answer whose full count equals the budget is not a sample", async () => {
  const r = await sampleIfCapped({ rows_total: 20000 }, 20000, async () => 20000, async () => { throw new Error("must not fetch"); });
  assert.equal(r.sampling, null);
  assert.equal(r.result.rows_total, 20000);
});

test("a capped answer is replaced by the thinned one and reports full count, shown count and modulus", async () => {
  let modulusSeen = 0;
  const r = await sampleIfCapped(
    { rows_total: 20000 },
    20000,
    async () => 87342,
    async (m) => { modulusSeen = m; return { rows_total: 10918 }; },
  );
  assert.equal(modulusSeen, 8);
  assert.deepEqual(r.sampling, { total: 87342, shown: 10918, modulus: 8, budget: 20000 });
  assert.equal(r.result.rows_total, 10918);
});
