import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { compile, initLowering } from "../src/engine/lowering.ts";
import { expandSourceSql, loadShards, mapLimit, shardFileName, summarizeIncomplete } from "../src/engine/shards.ts";
import type { PlanGraph, ShardBinding } from "../src/engine/contract.ts";

const bytes = (n: number) => new ArrayBuffer(n);
const fullShard = (id: string, url = `/${id}.parquet`): ShardBinding => ({ id, url, bytes: 10, access: "full" });

test("shard file names include the execution, source, and shard indexes", () => {
  assert.equal(shardFileName("gf7", 2, 3), "gf7_src_2_s3.parquet");
});

test("expandSourceSql expands a single shard and every occurrence", () => {
  const sql = "SELECT * FROM read_parquet('gf_src_0.parquet') UNION ALL SELECT * FROM read_parquet('gf_src_0.parquet')";
  assert.equal(
    expandSourceSql(sql, "gf_src_0.parquet", ["gf8_src_0_s0.parquet"]),
    "SELECT * FROM read_parquet(['gf8_src_0_s0.parquet']) UNION ALL SELECT * FROM read_parquet(['gf8_src_0_s0.parquet'])",
  );
});

test("expandSourceSql expands several SQL-escaped shard names", () => {
  assert.equal(
    expandSourceSql("read_parquet('logical.parquet')", "logical.parquet", ["one.parquet", "it's.parquet"]),
    "read_parquet(['one.parquet','it''s.parquet'])",
  );
});

test("expandSourceSql treats special characters literally and does not collide with prefix names", () => {
  const sql = "read_parquet('gf.src_[1].parquet') || read_parquet('gf.src_[1].parquet0')";
  assert.equal(
    expandSourceSql(sql, "gf.src_[1].parquet", ["s.parquet"]),
    "read_parquet(['s.parquet']) || read_parquet('gf.src_[1].parquet0')",
  );
  const prefixes = "read_parquet('gf_src_1.parquet') || read_parquet('gf_src_10.parquet')";
  assert.equal(
    expandSourceSql(prefixes, "gf_src_1.parquet", ["one.parquet"]),
    "read_parquet(['one.parquet']) || read_parquet('gf_src_10.parquet')",
  );
});

test("expandSourceSql requires the logical source literal", () => {
  assert.throws(() => expandSourceSql("SELECT 1", "missing.parquet", ["s.parquet"]), /absent/);
});

test("mapLimit bounds concurrency, preserves order, and isolates failures", async () => {
  let running = 0;
  let peak = 0;
  const results = await mapLimit([0, 1, 2, 3, 4], 2, async (value) => {
    running++;
    peak = Math.max(peak, running);
    await new Promise((resolve) => setTimeout(resolve, 2 + (4 - value)));
    running--;
    if (value === 2) throw new Error("two failed");
    return value * 10;
  });
  assert.equal(peak, 2);
  assert.deepEqual(results.map((result) => result.status), ["fulfilled", "fulfilled", "rejected", "fulfilled", "fulfilled"]);
  assert.deepEqual(results.map((result) => result.status === "fulfilled" ? result.value : (result.reason as Error).message), [0, 10, "two failed", 30, 40]);
});

test("mapLimit stops starting work after abort and marks unstarted items AbortError", async () => {
  const controller = new AbortController();
  const started: number[] = [];
  let release!: () => void;
  const gate = new Promise<void>((resolve) => { release = resolve; });
  const pending = mapLimit([0, 1, 2], 1, async (value) => {
    started.push(value);
    await gate;
    return value;
  }, controller.signal);
  await Promise.resolve();
  controller.abort();
  release();
  const results = await pending;
  assert.deepEqual(started, [0]);
  assert.equal(results[0].status, "fulfilled");
  for (const result of results.slice(1)) {
    assert.equal(result.status, "rejected");
    if (result.status === "rejected") assert.equal((result.reason as Error).name, "AbortError");
  }
});

test("mapLimit does not invoke callbacks reserved just before an immediate abort", async () => {
  const controller = new AbortController();
  const started: number[] = [];
  const pending = mapLimit([0, 1], 2, async (value) => { started.push(value); return value; }, controller.signal);
  controller.abort();
  const results = await pending;
  assert.deepEqual(started, []);
  assert.ok(results.every((result) => result.status === "rejected" && (result.reason as Error).name === "AbortError"));
});

test("loadShards loads whole-file shards with bounded concurrency and ignores range shards", async () => {
  const shards = [fullShard("a"), fullShard("b"), fullShard("c"), { id: "range", url: "/range", bytes: 20, access: "range" as const }];
  let running = 0;
  let peak = 0;
  const result = await loadShards(shards, {
    concurrency: 2,
    async getBytes() {
      running++;
      peak = Math.max(peak, running);
      await new Promise((resolve) => setTimeout(resolve, 2));
      running--;
      return bytes(4);
    },
  });
  assert.equal(peak, 2);
  assert.deepEqual(result.loaded.map(({ shard }) => shard.id), ["a", "b", "c"]);
  assert.ok(result.loaded.every(({ bytes: loaded }) => loaded.byteLength === 4));
  assert.deepEqual(result.failed, []);
});

test("loadShards returns successful downloads and failure reasons separately", async () => {
  const result = await loadShards([fullShard("ok"), fullShard("bad")], {
    concurrency: 2,
    getBytes: async (url) => {
      if (url.includes("bad")) throw new Error("HTTP 503");
      return bytes(5);
    },
  });
  assert.deepEqual(result.loaded.map(({ shard }) => shard.id), ["ok"]);
  assert.deepEqual(result.failed.map(({ shard, reason }) => [shard.id, reason]), [["bad", "HTTP 503"]]);
});

test("loadShards reports an abort in flight and does not start later work", async () => {
  const controller = new AbortController();
  const started: string[] = [];
  const pending = loadShards([fullShard("a"), fullShard("b"), fullShard("c")], {
    concurrency: 1,
    signal: controller.signal,
    getBytes: (url, signal) => new Promise((_resolve, reject) => {
      started.push(url);
      signal?.addEventListener("abort", () => reject(new DOMException("stopped", "AbortError")), { once: true });
    }),
  });
  await Promise.resolve();
  await Promise.resolve();
  controller.abort();
  const result = await pending;
  assert.deepEqual(started, ["/a.parquet"]);
  assert.deepEqual(result.loaded, []);
  assert.deepEqual(result.failed.map(({ shard }) => shard.id), ["a", "b", "c"]);
  assert.ok(result.failed.every(({ reason }) => /stopped|aborted/.test(reason)));
});

test("summarizeIncomplete is absent for success and identifies failed shards", () => {
  assert.equal(summarizeIncomplete([], 3), undefined);
  assert.deepEqual(summarizeIncomplete([{ shard: fullShard("bad"), reason: "HTTP 500" }], 3), {
    failed_shards: [{ id: "bad", reason: "HTTP 500" }],
    total_shards: 3,
  });
});

await initLowering(readFileSync(new URL("../src/engine/generated/geo_lowering.wasm", import.meta.url)));

test("compile carries shard bindings while retaining the logical Parquet literal", () => {
  const plan: PlanGraph = {
    ir_version: "0.3",
    nodes: { source: { op: "source", uri: "logical.parquet", format: "geo_parquet", schema: { fields: [], geometry_columns: [] } } },
    root: "source",
  };
  const shards = [fullShard("a"), { id: "b", url: "/b.parquet", bytes: 20, access: "range" as const }];
  const compiled = compile(plan, { "logical.parquet": { url: shards[0].url, access: shards[0].access, covering: null, shards } }, 10);
  assert.strictEqual(compiled.files[0].shards, shards);
  assert.match(compiled.sql, /read_parquet\('gf_src_0\.parquet'\)/);
  assert.equal(compiled.files[0].name, "gf_src_0.parquet");
});
