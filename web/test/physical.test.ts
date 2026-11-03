import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import type { PlanGraph } from "../src/engine/contract.ts";
import { buildPlan, initLowering } from "../src/engine/lowering.ts";
import { PartitionCatalog, requiredExtents, resolveSources, shardAccess, toBindings, type ResolvedSource } from "../src/static/physical.ts";
import { cellIdForPoint, type DatasetPartitions, type PartitionEntry } from "../src/static/partitions.ts";
import type { BBox } from "../src/vietnam.ts";

await initLowering(readFileSync(new URL("../src/engine/generated/geo_lowering.wasm", import.meta.url)));

const schema = { fields: [], geometry_columns: [] };
const source = (uri: string) => ({ op: "source", uri, format: "geo_parquet", schema });
const bbox = (input: string, box: BBox, crs: unknown = "ogc_crs84") => ({
  op: "bbox_filter", input, geometry_column: "geometry",
  bbox: { min_x: box[0], min_y: box[1], max_x: box[2], max_y: box[3], crs },
});
const graph = (nodes: Record<string, unknown>, root: string): PlanGraph => ({ ir_version: "0.3", nodes, root, hints: {} } as PlanGraph);

test("requiredExtents follows simple and transparent source prefixes", () => {
  const simple = graph({ s: source("places"), b: bbox("s", [106.6, 10.7, 106.8, 10.9]) }, "b");
  assert.deepEqual(requiredExtents(simple), { places: [106.6, 10.7, 106.8, 10.9] });

  const transparent = graph({
    s: source("places"),
    f: { op: "filter", input: "s", predicate: { kind: "is_null", column: "name" } },
    p: { op: "project", input: "f", columns: ["name", "geometry"] },
    b: bbox("p", [105, 20, 106, 21], { epsg: 4326 }),
  }, "b");
  assert.deepEqual(requiredExtents(transparent), { places: [105, 20, 106, 21] });
});

test("requiredExtents intersects nested boxes and represents an empty intersection as a degenerate box", () => {
  const nested = graph({ s: source("x"), a: bbox("s", [0, 0, 10, 10]), b: bbox("a", [2, 3, 8, 9]) }, "b");
  assert.deepEqual(requiredExtents(nested).x, [2, 3, 8, 9]);
  const empty = graph({ s: source("x"), a: bbox("s", [0, 0, 1, 1]), b: bbox("a", [5, 5, 6, 6]) }, "b");
  assert.deepEqual(requiredExtents(empty).x, [5, 5, 5, 5]);
});

test("requiredExtents stops at aggregate and limit before a later bbox", () => {
  const aggregate = graph({
    s: source("x"),
    a: { op: "aggregate", input: "s", group_by: [], aggregates: [{ func: "count", column: null, alias: "count" }] },
    b: bbox("a", [1, 2, 3, 4]),
  }, "b");
  const limit = graph({ s: source("x"), l: { op: "limit", input: "s", n: 10 }, b: bbox("l", [1, 2, 3, 4]) }, "b");
  assert.deepEqual(requiredExtents(aggregate), { x: null });
  assert.deepEqual(requiredExtents(limit), { x: null });
});

test("requiredExtents keeps independent left and right spatial-join windows", () => {
  const plan = graph({
    left: source("buildings"), leftBox: bbox("left", [106.69, 10.77, 106.71, 10.79]),
    right: source("roads"), rightBox: bbox("right", [106.68, 10.76, 106.72, 10.80]),
    join: { op: "spatial_join", left: "leftBox", right: "rightBox" },
  }, "join");
  assert.deepEqual(requiredExtents(plan), {
    buildings: [106.69, 10.77, 106.71, 10.79],
    roads: [106.68, 10.76, 106.72, 10.80],
  });
});

test("requiredExtents unions branches of a shared source and an unbounded branch wins", () => {
  const shared = graph({
    s: source("x"), a: bbox("s", [0, 0, 1, 1]), b: bbox("s", [4, 5, 6, 7]),
    join: { op: "spatial_join", left: "a", right: "b" },
  }, "join");
  assert.deepEqual(requiredExtents(shared), { x: [0, 0, 6, 7] });

  const unbounded = graph({
    s: source("x"), a: bbox("s", [0, 0, 1, 1]), l: { op: "limit", input: "s", n: 1 },
    join: { op: "spatial_join", left: "a", right: "l" },
  }, "join");
  assert.deepEqual(requiredExtents(unbounded), { x: null });
});

test("requiredExtents treats non-degree and absent boxes as unbounded and never mutates the plan", () => {
  const nonDegree = graph({ s: source("x"), b: bbox("s", [0, 0, 1, 1], { epsg: 3857 }) }, "b");
  const untouched = structuredClone(nonDegree);
  assert.deepEqual(requiredExtents(nonDegree), { x: null });
  assert.deepEqual(nonDegree, untouched);
  assert.deepEqual(requiredExtents(graph({ s: source("x"), p: { op: "project", input: "s", columns: [] } }, "p")), { x: null });
});

test("requiredExtents reads viewport bounds produced by the canonical wasm builder", () => {
  const sources = JSON.parse(readFileSync(new URL("../../crates/geo-plan/fixtures/requests/_sources.json", import.meta.url), "utf8")) as Record<string, unknown>;
  const built = buildPlan({ kind: "features", source: "buildings", bbox: [106.69, 10.77, 106.71, 10.79], crs: "OGC:CRS84" }, sources).plan;
  assert.deepEqual(requiredExtents(built), { "demo/buildings.parquet": [106.69, 10.77, 106.71, 10.79] });
});

test("requiredExtents reads distinct metric-with bounds produced by the canonical wasm builder", () => {
  const sources = JSON.parse(readFileSync(new URL("../../crates/geo-plan/fixtures/requests/_sources.json", import.meta.url), "utf8")) as Record<string, unknown>;
  const built = buildPlan({
    kind: "near", source: "buildings", bbox: [106.69, 10.77, 106.71, 10.79], crs: "OGC:CRS84",
    with: { source: "roads", distance_m: 200, filter: { column: "class", op: "in", values: ["motorway", "trunk", "primary"] } },
  }, sources).plan;
  const extents = requiredExtents(built);
  assert.deepEqual(extents["demo/buildings.parquet"], [106.69, 10.77, 106.71, 10.79]);
  const roads = extents["demo/roads.parquet"]!;
  assert.ok(roads[0] < 106.69 && roads[1] < 10.77 && roads[2] > 106.71 && roads[3] > 10.79, `${roads}`);
  assert.deepEqual(roads, [106.68813445116137, 10.768155081664768, 106.71186554883862, 10.791844918335231]);
});

function regionManifest(dataset: string, west: number, south: number): DatasetPartitions {
  const partitions: PartitionEntry[] = [];
  let i = 0;
  for (let y = 0; y < 2; y++) for (let x = 0; x < 3; x++) {
    const box: BBox = [west + x * 0.012, south + y * 0.012, west + x * 0.012 + 0.009, south + y * 0.012 + 0.009];
    const id = cellIdForPoint((box[0] + box[2]) / 2, (box[1] + box[3]) / 2, 13);
    const bytes = [2_000_000, 700_000, 3_000_000, 1_500_000, 4_000_000, 2_500_000][i];
    partitions.push({
      id, bbox: box, url: `https://cdn.example/owner/data@0123456789abcdef0123456789abcdef01234567/v1/${dataset}/${id}.parquet`,
      bytes, sha256: (i + 1).toString(16).padStart(64, "0"), format: "GeoParquet", crs: "OGC:CRS84", rows: 100 + i,
      license: "ODbL-1.0", source_release: "2026-09-23.1",
    });
    i++;
  }
  return {
    schema: 3, dataset, data_version: "v1", partition_scheme: "quadtree-v1", min_zoom: 12,
    rows: partitions.reduce((sum, part) => sum + part.rows, 0),
    bbox: [west, south, west + 0.033, south + 0.021], partitions,
  };
}

const hanoi = regionManifest("hanoi-buildings", 105.80, 21.00);
const hcmc = regionManifest("hcmc-buildings", 106.68, 10.76);
const clone = <T>(value: T): T => structuredClone(value);

function catalogWith(manifests: Record<string, DatasetPartitions>): PartitionCatalog {
  const catalog = new PartitionCatalog(async (url) => clone(manifests[url]));
  for (const id of Object.keys(manifests)) catalog.register(id, { manifest_url: id });
  return catalog;
}

test("PartitionCatalog shares concurrent loads and builds one cached result", async () => {
  let calls = 0;
  let release!: () => void;
  const gate = new Promise<void>((resolve) => { release = resolve; });
  const catalog = new PartitionCatalog(async () => { calls++; await gate; return clone(hanoi); });
  catalog.register("hn", { manifest_url: "hn.json" });
  assert.equal(catalog.has("hn"), true);
  const first = catalog.load("hn");
  const second = catalog.load("hn");
  await Promise.resolve();
  assert.equal(calls, 1);
  release();
  const [a, b] = await Promise.all([first, second]);
  assert.strictEqual(a, b);
  assert.strictEqual(await catalog.load("hn"), a);
  assert.equal(calls, 1);
});

test("PartitionCatalog does not cache failures", async () => {
  let calls = 0;
  const catalog = new PartitionCatalog(async () => {
    calls++;
    if (calls === 1) throw new Error("temporary");
    return clone(hanoi);
  });
  catalog.register("hn", { manifest_url: "hn.json" });
  await assert.rejects(catalog.load("hn"), /temporary/);
  assert.equal((await catalog.load("hn")).manifest.dataset, "hanoi-buildings");
  assert.equal(calls, 2);
});

test("PartitionCatalog reports validation problems", async () => {
  const invalid = clone(hanoi) as DatasetPartitions & { schema: number };
  invalid.schema = 2;
  invalid.data_version = "latest";
  const catalog = new PartitionCatalog(async () => invalid);
  catalog.register("bad", { manifest_url: "bad.json" });
  await assert.rejects(catalog.load("bad"), (error: unknown) => {
    assert.match((error as Error).message, /schema 2 is not 3/);
    assert.match((error as Error).message, /never 'latest'/);
    return true;
  });
});

test("PartitionCatalog gives each caller abort semantics without poisoning a shared load", async () => {
  let release!: () => void;
  const gate = new Promise<void>((resolve) => { release = resolve; });
  const catalog = new PartitionCatalog(async () => { await gate; return clone(hanoi); });
  catalog.register("hn", { manifest_url: "hn.json" });

  const already = new AbortController();
  already.abort();
  await assert.rejects(catalog.load("hn", already.signal), { name: "AbortError" });

  const active = new AbortController();
  const cancelled = catalog.load("hn", active.signal);
  const survivor = catalog.load("hn");
  active.abort();
  await assert.rejects(cancelled, { name: "AbortError" });
  release();
  assert.equal((await survivor).manifest.dataset, "hanoi-buildings");
});

test("PartitionCatalog rejects an unknown dataset clearly", async () => {
  const catalog = new PartitionCatalog(async () => clone(hanoi));
  await assert.rejects(catalog.load("missing"), /unknown partitioned dataset: missing/);
});

function bboxPlan(uri: string, box: BBox): PlanGraph {
  return graph({ s: source(uri), b: bbox("s", box) }, "b");
}

test("resolveSources isolates Hanoi and HCMC for one-shard and multi-shard windows", async () => {
  const catalog = catalogWith({ hn: hanoi, hcmc });
  const cases: Array<[DatasetPartitions, string, BBox]> = [
    [hanoi, "hn", [105.801, 21.001, 105.802, 21.002]],
    [hcmc, "hcmc", [106.681, 10.761, 106.682, 10.762]],
  ];
  for (const [manifest, id, one] of cases) {
    const own = await resolveSources(bboxPlan("logical", one), { logical: id }, catalog, { zoom: 13 });
    assert.equal(own.logical.state, "ok");
    assert.equal(own.logical.shards.length, 1);
    const otherId = id === "hn" ? "hcmc" : "hn";
    const other = await resolveSources(bboxPlan("logical", one), { logical: otherId }, catalog, { zoom: 13 });
    assert.equal(other.logical.state, "empty");
    assert.equal(other.logical.shards.length, 0);

    const many = await resolveSources(bboxPlan("logical", manifest.bbox), { logical: id }, catalog, { zoom: 13 });
    assert.equal(many.logical.state, "ok");
    assert.equal(many.logical.shards.length, manifest.partitions.length);
    const farId = id === "hn" ? "hcmc" : "hn";
    const far = await resolveSources(bboxPlan("logical", manifest.bbox), { logical: farId }, catalog, { zoom: 13 });
    assert.equal(far.logical.state, "empty");
  }
});

test("resolveSources applies zoom gating while null zoom skips it", async () => {
  const catalog = catalogWith({ hn: hanoi });
  const plan = bboxPlan("logical", [105.801, 21.001, 105.802, 21.002]);
  const gated = await resolveSources(plan, { logical: "hn" }, catalog, { zoom: 11 });
  assert.equal(gated.logical.state, "gated");
  assert.match(gated.logical.reason!, /zoom 12/);
  const script = await resolveSources(plan, { logical: "hn" }, catalog, { zoom: null });
  assert.equal(script.logical.state, "ok");
});

test("resolveSources gates an unbounded source over the cap and reads it under the cap", async () => {
  const catalog = catalogWith({ hn: hanoi });
  const plan = graph({ s: source("logical") }, "s");
  const gated = await resolveSources(plan, { logical: "hn" }, catalog, { zoom: null }, { maxShards: 5 });
  assert.equal(gated.logical.state, "gated");
  assert.match(gated.logical.reason!, /needs the whole dataset/);
  const all = await resolveSources(plan, { logical: "hn" }, catalog, { zoom: null }, { maxShards: 6 });
  assert.equal(all.logical.state, "ok");
  assert.equal(all.logical.shards.length, 6);
  assert.equal(all.logical.estimatedBytes, hanoi.partitions.reduce((sum, part) => sum + part.bytes, 0));
});

test("resolveSources truncates nearest-first deterministically and sums selected bytes", async () => {
  const catalog = catalogWith({ hn: hanoi });
  const plan = bboxPlan("logical", hanoi.bbox);
  const first = await resolveSources(plan, { logical: "hn" }, catalog, { zoom: 13 }, { maxShards: 2 });
  const second = await resolveSources(plan, { logical: "hn" }, catalog, { zoom: 13 }, { maxShards: 2 });
  assert.equal(first.logical.state, "truncated");
  assert.equal(first.logical.shards.length, 2);
  assert.deepEqual(first.logical.shards.map((part) => part.id), second.logical.shards.map((part) => part.id));
  assert.equal(first.logical.estimatedBytes, first.logical.shards.reduce((sum, part) => sum + part.bytes, 0));
  const centre: [number, number] = [(hanoi.bbox[0] + hanoi.bbox[2]) / 2, (hanoi.bbox[1] + hanoi.bbox[3]) / 2];
  const distance = (part: PartitionEntry) => Math.hypot((part.bbox[0] + part.bbox[2]) / 2 - centre[0], (part.bbox[1] + part.bbox[3]) / 2 - centre[1]);
  const expected = [...hanoi.partitions].sort((a, b) => distance(a) - distance(b) || a.id.localeCompare(b.id)).slice(0, 2).map((part) => part.id);
  assert.deepEqual(first.logical.shards.map((part) => part.id), expected);
});

test("resolveSources returns the smallest shard as an empty-view probe", async () => {
  const catalog = catalogWith({ hn: hanoi });
  const empty = await resolveSources(bboxPlan("logical", [108, 15, 108.1, 15.1]), { logical: "hn" }, catalog, { zoom: 13 });
  assert.equal(empty.logical.state, "empty");
  assert.deepEqual(empty.logical.shards, []);
  assert.equal(empty.logical.probe?.bytes, 700_000);
  assert.equal(empty.logical.estimatedBytes, 700_000);
});

test("resolveSources resolves two sources independently and ignores an unmapped source", async () => {
  const catalog = catalogWith({ hn: hanoi, hcmc });
  const plan = graph({
    hs: source("hanoi-uri"), hb: bbox("hs", [105.801, 21.001, 105.802, 21.002]),
    ss: source("hcmc-uri"), sb: bbox("ss", [106.681, 10.761, 106.682, 10.762]),
    ignored: source("not-partitioned"),
    join: { op: "spatial_join", left: "hb", right: "sb" },
  }, "join");
  const resolved = await resolveSources(plan, { "hanoi-uri": "hn", "hcmc-uri": "hcmc" }, catalog, { zoom: 13 });
  assert.deepEqual(Object.keys(resolved).sort(), ["hanoi-uri", "hcmc-uri"]);
  assert.equal(resolved["hanoi-uri"].dataset, "hn");
  assert.equal(resolved["hcmc-uri"].dataset, "hcmc");
  assert.equal(resolved["hanoi-uri"].shards.length, 1);
  assert.equal(resolved["hcmc-uri"].shards.length, 1);
});

test("shardAccess pins the 1 MiB and range-host rule", () => {
  assert.equal(shardAccess(1024 * 1024, true), "full");
  assert.equal(shardAccess(1024 * 1024 + 1, true), "range");
  assert.equal(shardAccess(50_000_000, false), "full");
});

test("toBindings keeps compatibility fields on the first shard and emits every shard", () => {
  const resolved: ResolvedSource = {
    uri: "logical", dataset: "hn", state: "ok", shards: [hanoi.partitions[1], hanoi.partitions[0]],
    estimatedBytes: 2_700_000, extent: hanoi.bbox, minZoom: 12,
  };
  const binding = toBindings(resolved, true);
  assert.equal(binding.url, hanoi.partitions[1].url);
  assert.equal(binding.access, "full");
  assert.deepEqual(binding.shards, [
    { id: hanoi.partitions[1].id, url: hanoi.partitions[1].url, bytes: 700_000, access: "full" },
    { id: hanoi.partitions[0].id, url: hanoi.partitions[0].url, bytes: 2_000_000, access: "range" },
  ]);

  const probeOnly = { ...resolved, state: "empty" as const, shards: [], probe: hanoi.partitions[1], estimatedBytes: 700_000 };
  assert.equal(toBindings(probeOnly, true).shards.length, 1);
});
