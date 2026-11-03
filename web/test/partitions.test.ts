import assert from "node:assert/strict";
import { test } from "node:test";
import { buildIndex, selectForView, shardsInView, validateDatasetPartitions, type DatasetPartitions, type PartitionEntry } from "../src/static/partitions.ts";
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
  const parts: PartitionEntry[] = l.map((b, i) => ({
    id: l.ids![i], bbox: b, url: `https://cdn.example/gh/owner/data@0123456789abcdef0123456789abcdef01234567/v2/buildings/${l.ids![i]}.parquet`,
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
