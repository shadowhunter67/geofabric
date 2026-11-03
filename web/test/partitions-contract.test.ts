import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import {
  buildIndex,
  cellBBox,
  cellIdForPoint,
  partitionStats,
  shardsInView,
  validateDatasetPartitions,
  type DatasetPartitions,
} from "../src/static/partitions.ts";
import type { BBox } from "../src/vietnam.ts";

const fixture = <T>(name: string): T => JSON.parse(readFileSync(new URL(`fixtures/partitions/${name}`, import.meta.url), "utf8")) as T;
const manifest = fixture<DatasetPartitions>("places-build.partitions.json");

interface CellGolden {
  cells: Record<string, BBox>;
  points_depth13: Array<{ x: number; y: number; id: string }>;
}

const overlaps = (a: BBox, b: BBox) => a[0] <= b[2] && a[2] >= b[0] && a[1] <= b[3] && a[3] >= b[1];
const bruteIds = (m: DatasetPartitions, view: BBox) => m.partitions.filter((p) => overlaps(p.bbox, view)).map((p) => p.id).sort();

test("TypeScript quadtree cells and point ownership exactly match the Python golden file", () => {
  const golden = fixture<CellGolden>("cell-golden.json");
  for (const [id, expected] of Object.entries(golden.cells)) {
    const actual = cellBBox(id);
    assert.equal(actual.length, expected.length);
    actual.forEach((value, i) => assert.equal(value, expected[i], `${id}[${i}]`));
  }
  for (const point of golden.points_depth13) assert.equal(cellIdForPoint(point.x, point.y, 13), point.id, JSON.stringify(point));
});

test("the real build manifest passes build validation and fails published validation only for its 19 URLs", () => {
  assert.deepEqual(validateDatasetPartitions(manifest, { stage: "build" }), []);
  const published = validateDatasetPartitions(manifest);
  assert.equal(published.length, manifest.partitions.length);
  assert.ok(published.every((problem) => problem.includes("url must be an https URL pinned")), published.join("\n"));
});

test("real-manifest lookup matches inclusive brute force for 12 boundary and coverage windows", () => {
  const windows: Array<{ name: string; box: BBox; count?: number }> = [
    { name: "tiny inside one shard", box: [106.63, 10.755, 106.635, 10.76], count: 1 },
    { name: "straddles two shards", box: [106.3499984741211, 10.350028038024902, 106.3499984741211, 10.500086784362793], count: 2 },
    { name: "straddles three shards", box: [106.3499984741211, 10.875009536743164, 106.49999237060547, 11], count: 3 },
    { name: "straddles four shards", box: [106.42498016357422, 10.350028038024902, 106.50005340576172, 10.500086784362793], count: 4 },
    { name: "whole dataset", box: manifest.bbox },
    { name: "outside dataset", box: [100, 1, 101, 2], count: 0 },
    { name: "touches exactly a shard edge", box: [106.49996185302734, 10.4, 106.49996185302734, 10.4], count: 1 },
    { name: "only a straddling feature overhang", box: [106.49999237060547, 10.875009536743164, 106.49999237060547, 10.875009536743164], count: 1 },
    { name: "dense core", box: [106.65, 10.77, 106.7, 10.82] },
    { name: "eastern fringe", box: [106.99, 10.6, 107.02, 10.9] },
    { name: "northern band", box: [106.4, 11.05, 107.01, 11.2] },
    { name: "southern band", box: [106.4, 10.36, 106.9, 10.49] },
  ];
  const index = buildIndex(manifest.partitions);
  const reversed = buildIndex([...manifest.partitions].reverse());
  for (const { name, box, count } of windows) {
    const expected = bruteIds(manifest, box);
    if (count != null) assert.equal(expected.length, count, name);
    assert.deepEqual(shardsInView(index, box).map((p) => p.id), expected, name);
    assert.deepEqual(shardsInView(reversed, box).map((p) => p.id), expected, `${name}, reversed`);
  }
  assert.equal(shardsInView(index, manifest.bbox).length, manifest.partitions.length);
});

test("partition statistics summarize the real fixture", () => {
  const stats = partitionStats(manifest);
  assert.equal(stats.count, 19);
  assert.equal(stats.rows, 449_849);
  assert.ok(stats.bytes.max <= 5_000_000);
  assert.ok(stats.bytes.min > 0);
});

test("real-manifest contract mutations are all detected", () => {
  const mutations: Array<[string, (m: DatasetPartitions) => void, RegExp]> = [
    ["swapped ids", (m) => { [m.partitions[0].id, m.partitions[1].id] = [m.partitions[1].id, m.partitions[0].id]; }, /build url|quadtree cell/],
    ["bad sha256", (m) => { m.partitions[0].sha256 = "not-a-sha"; }, /sha256/],
    ["NaN bbox", (m) => { m.partitions[0].bbox[0] = Number.NaN; }, /bbox is not a valid box/],
    ["missing partition", (m) => { m.partitions.pop(); }, /partition rows add up/],
    ["parent path", (m) => { m.partitions[0].url = "../x.parquet"; }, /build url must be exactly/],
    ["wrong CRS", (m) => { m.partitions[0].crs = "EPSG:3857"; }, /allowed CRS/],
    ["reversed zoom range", (m) => { m.partitions[0].min_zoom = 12; m.partitions[0].max_zoom = 5; }, /min_zoom cannot exceed max_zoom/],
  ];
  for (const [name, mutate, expected] of mutations) {
    const changed = structuredClone(manifest);
    mutate(changed);
    assert.match(validateDatasetPartitions(changed, { stage: "build" }).join("\n"), expected, name);
  }
});
