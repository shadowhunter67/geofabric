import assert from "node:assert/strict";
import { test } from "node:test";
import { cellString, resultDigest } from "../src/engine/digest.ts";

test("cells are rendered like the Rust canonical digest", () => {
  assert.equal(cellString(10n), "10");
  assert.equal(cellString(3), "3");
  assert.equal(cellString(true), "true");
  assert.equal(cellString(false), "false");
  assert.equal(cellString(null), "NULL");
  assert.equal(cellString("x y"), "x y");
  assert.equal(cellString(new Uint8Array([0, 10, 255])), "000aff");
});

test("digest matches the value computed independently with python hashlib", async () => {
  // python: sha256("\n".join(sorted(["1|a|0a0b","10|b|NULL","2|c|true"])))
  const rows = [
    [2n, "c", true],
    [1n, "a", new Uint8Array([0x0a, 0x0b])],
    [10n, "b", null],
  ];
  const d = await resultDigest(rows);
  assert.equal(d.count, 3);
  assert.equal(d.sha256, "02a30dfc02dd3d33332113a9fa9c871087040c7657d8c5f8d2cf04db6c7f5fb5");
});

test("a scalar integer result is a count query: count = the value", async () => {
  const d = await resultDigest([[5n]]);
  assert.equal(d.count, 5);
  assert.equal(d.sha256, "ef2d127de37b942baad06145e54b0c619a1f22327b2ebbcfbec78f5564afe39d");
  // a single row with a string is not a count
  assert.equal((await resultDigest([["x"]])).count, 1);
});

test("row order does not change the digest", async () => {
  const a = await resultDigest([[1n], [2n], [3n]]);
  const b = await resultDigest([[3n], [1n], [2n]]);
  assert.equal(a.sha256, b.sha256);
});

test("floats use the shortest round-trip spelling (same as Rust Display)", async () => {
  const d = await resultDigest([[1.5], [4], [0.1]], ["Float64"]);
  assert.equal(d.count, 3);
  assert.equal(d.sha256, "1607ef75523fb4893a4bf4aa248381a9b62595e6a987d6bf06f517c1182811c8");
  // a single float is a row, not a count
  assert.equal((await resultDigest([[4]], ["Float64"])).count, 1);
  assert.equal((await resultDigest([[4n]], ["Int64"])).count, 4);
});
