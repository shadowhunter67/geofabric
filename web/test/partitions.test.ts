import assert from "node:assert/strict";
import { test } from "node:test";
import { ALLOWED_CRS, MAX_SHARD_BYTES, buildIndex, cellBBox, cellIdForPoint, partitionStats, selectForView, shardsInView, validateDatasetPartitions, type DatasetPartitions, type PartitionEntry } from "../src/static/partitions.ts";
import type { BBox } from "../src/vietnam.ts";

// A synthetic quadtree over a Vietnam-sized root: dense corners are split deeper, like real building density.
const ROOT: BBox = [102, 8, 110, 24];
function leaves(depth: number, dense: (b: BBox) => boolean, b: BBox = ROOT, id = "q"): BBox[] & { ids?: string[] } {
  const out: { b: BBox; id: string }[] = [];
  const walk = (box: BBox, key: string, d: number) => {
    if (d >= depth || !dense(box)) { out.push({ b: box, id: key }); return; }
    const mx = (box[0] + box[2]) / 2, my = (box[1] + box[3]) / 2;
    walk([box[0], box[1], mx, my], key + "0", d + 1); walk([mx, box[1], box[2], my], key + "1", d + 1);
    walk([box[0], my, mx, box[3]], key + "2", d + 1); walk([mx, my, box[2], box[3]], key + "3", d + 1);
  };
  walk(b, id, 0);
  const r = out.map((o) => o.b) as BBox[] & { ids?: string[] };
  r.ids = out.map((o) => o.id);
  return r;
}
const hanoi: BBox = [105.7, 20.9, 106.0, 21.15];
const hcmc: BBox = [106.55, 10.65, 106.9, 10.95];
const dense = (b: BBox) => [hanoi, hcmc].some((c) => b[0] <= c[2] && b[2] >= c[0] && b[1] <= c[3] && b[3] >= c[1]);

function fixture(): DatasetPartitions {
  const l = leaves(9, dense);
  const ids = l.map((b) => cellIdForPoint((b[0] + b[2]) / 2, (b[1] + b[3]) / 2, 13));
  const parts: PartitionEntry[] = l.map((b, i) => ({
    id: ids[i], bbox: b, url: `https://cdn.example/gh/owner/data@0123456789abcdef0123456789abcdef01234567/v2/buildings/${ids[i]}.parquet`,
    bytes: 5_000_000 + i, sha256: i.toString(16).padStart(64, "0"), format: "GeoParquet", crs: "OGC:CRS84", rows: 10_000 + i,
    license: "ODbL-1.0", source_release: "2026-09-23.1",
  }));
  return { schema: 3, dataset: "buildings", data_version: "v2", partition_scheme: "quadtree-v1", min_zoom: 13, rows: parts.reduce((s, p) => s + p.rows, 0), bbox: ROOT, partitions: parts };
}

test("a well-formed dataset manifest has no problems", () => {
  assert.deepEqual(validateDatasetPartitions(fixture()), []);
});

test("CI rules: duplicate ids, bad checksums, mutable URLs, outliers, inconsistent counts are all reported", () => {
  const m = fixture();
  m.partitions[1].id = m.partitions[0].id;
  m.partitions[2].sha256 = "xyz";
  m.partitions[3].url = "https://cdn.example/gh/owner/data@main/v2/x.parquet";
  m.partitions[4].bbox = [10, 10, 20, 20];
  m.partitions[5].bytes = 0;
  m.rows += 1;
  m.data_version = "latest";
  const problems = validateDatasetPartitions(m).join("\n");
  for (const needle of ["duplicate partition id", "sha256 is not 64", "pinned to a commit", "sanity box", "bytes must be positive", "add up to", "never 'latest'"]) assert.match(problems, new RegExp(needle), needle);
});

test("partition contract rules accept finite limits and reject invalid numbers, sizes, zooms, ids, kinds, cells, CRS, and union extents", () => {
  const valid = fixture();
  valid.partitions[0].kind = "regular";
  valid.partitions[0].density = 1;
  valid.partitions[0].min_zoom = 0;
  valid.partitions[0].max_zoom = 22;
  assert.deepEqual(validateDatasetPartitions(valid), []);
  assert.deepEqual(ALLOWED_CRS, ["OGC:CRS84"]);

  const cases: Array<[string, (m: DatasetPartitions) => void, RegExp]> = [
    ["NaN bbox", (m) => { m.partitions[0].bbox[0] = Number.NaN; }, /bbox is not a valid box/],
    ["infinite bytes", (m) => { m.partitions[0].bytes = Infinity; }, /bytes must be positive and finite/],
    ["NaN rows", (m) => { m.partitions[0].rows = Number.NaN; }, /rows must be positive and finite/],
    ["infinite density", (m) => { m.partitions[0].density = Infinity; }, /density must be finite/],
    ["NaN zoom", (m) => { m.partitions[0].min_zoom = Number.NaN; }, /min_zoom must be finite/],
    ["large shard", (m) => { m.partitions[0].bytes = MAX_SHARD_BYTES + 1; }, /shard limit/],
    ["more rows than bytes", (m) => { m.partitions[0].bytes = m.partitions[0].rows - 1; }, /rows cannot exceed bytes/],
    ["zoom outside range", (m) => { m.partitions[0].max_zoom = 23; }, /not a map zoom/],
    ["reversed zoom range", (m) => { m.partitions[0].min_zoom = 12; m.partitions[0].max_zoom = 5; }, /min_zoom cannot exceed max_zoom/],
    ["invalid id", (m) => { m.partitions[0].id = "q4"; }, /id must match/],
    ["wrong kind", (m) => { m.partitions[0].kind = "oversize"; }, /disagrees with its id prefix/],
    ["wrong cell", (m) => { m.partitions[0].id = "q3333333333333"; }, /does not intersect its quadtree cell/],
    ["wrong CRS", (m) => { m.partitions[0].crs = "EPSG:3857"; }, /allowed CRS/],
    ["wrong union", (m) => { m.bbox[0] -= 0.25; }, /does not equal the union/],
  ];
  for (const [name, mutate, expected] of cases) {
    const m = fixture();
    mutate(m);
    assert.match(validateDatasetPartitions(m).join("\n"), expected, name);
  }
});

test("build URLs are exact dataset/id paths and published URLs remain immutable https URLs", () => {
  const build = fixture();
  for (const p of build.partitions) p.url = `${build.dataset}/${p.id}.parquet`;
  assert.deepEqual(validateDatasetPartitions(build, { stage: "build" }), []);
  build.partitions[0].url = "../x.parquet";
  assert.match(validateDatasetPartitions(build, { stage: "build" }).join("\n"), /build url must be exactly/);
  assert.equal(validateDatasetPartitions(fixture()).some((problem) => problem.includes("url")), false);
});

test("quadtree helpers validate ids and boundaries, and partition statistics use nearest-rank p90", () => {
  assert.deepEqual(cellBBox("q03"), [104, 8, 112, 16]);
  assert.deepEqual(cellBBox("L03"), [104, 8, 112, 16]);
  assert.equal(cellIdForPoint(128, 32, 2), "q33");
  assert.throws(() => cellBBox("q4"));
  assert.throws(() => cellIdForPoint(95, 0, 1));
  assert.throws(() => cellIdForPoint(100, 10, 14));
  const m = fixture();
  const stats = partitionStats(m);
  const sorted = m.partitions.map((p) => p.bytes).sort((a, b) => a - b);
  assert.equal(stats.count, m.partitions.length);
  assert.equal(stats.regular, m.partitions.length);
  assert.equal(stats.oversize, 0);
  assert.equal(stats.bytes.p90, sorted[Math.ceil(sorted.length * 0.9) - 1]);
  assert.equal(stats.rows, m.rows);
});

test("viewport lookup: exactly the shards whose data extent meets the view, in id order, and the same whatever order the manifest lists them in", () => {
  const m = fixture();
  const idx = buildIndex(m.partitions);
  const view: BBox = [106.700, 10.750, 106.708, 10.757]; // a few blocks
  const hit = shardsInView(idx, view);
  assert.ok(hit.length >= 1 && hit.length <= 4, `${hit.length} shards for a neighbourhood`);
  const brute = m.partitions.filter((p) => p.bbox[0] <= view[2] && p.bbox[2] >= view[0] && p.bbox[1] <= view[3] && p.bbox[3] >= view[1]).map((p) => p.id).sort();
  assert.deepEqual(hit.map((p) => p.id), brute);
  const shuffled = buildIndex([...m.partitions].reverse());
  assert.deepEqual(shardsInView(shuffled, view).map((p) => p.id), hit.map((p) => p.id));
});

test("startup and regional views never open building shards (zoom policy comes before the lookup)", () => {
  const idx = buildIndex(fixture().partitions);
  const country = selectForView(idx, 13, [102, 8, 110, 24], 4.6);
  assert.deepEqual(country, { shards: [], gated: true, truncated: false });
  const region = selectForView(idx, 13, [105, 20, 107, 22], 8);
  assert.equal(region.gated, true);
});

test("a city view reads a handful of shards, and a view that would need too many is truncated around its centre, never silently complete", () => {
  const idx = buildIndex(fixture().partitions);
  const city = selectForView(idx, 13, hanoi, 13.5);
  assert.equal(city.gated, false);
  assert.ok(city.shards.length > 0 && city.shards.length <= 24, `${city.shards.length}`);
  const wide = selectForView(idx, 13, [102, 8, 110, 24], 13, 8);
  assert.equal(wide.truncated, true);
  assert.equal(wide.shards.length, 8);
  const again = selectForView(idx, 13, [102, 8, 110, 24], 13, 8);
  assert.deepEqual(again.shards.map((p) => p.id), wide.shards.map((p) => p.id)); // deterministic
});

test("lookup stays fast on a national-size manifest (thousands of shards)", () => {
  const many = leaves(6, () => true); // 4^6 = 4096 shards
  assert.ok(many.length >= 4000);
  const parts: PartitionEntry[] = many.map((b, i) => ({ ...fixture().partitions[0], id: `n${i}`, bbox: b }));
  const t0 = performance.now();
  const idx = buildIndex(parts);
  const build = performance.now() - t0;
  const t1 = performance.now();
  for (let i = 0; i < 1000; i++) shardsInView(idx, [105 + (i % 50) * 0.01, 10 + (i % 40) * 0.01, 105.1 + (i % 50) * 0.01, 10.1 + (i % 40) * 0.01]);
  const per = (performance.now() - t1) / 1000;
  assert.ok(build < 500, `index build ${build.toFixed(1)} ms`);
  assert.ok(per < 1, `lookup ${per.toFixed(3)} ms`);
});
