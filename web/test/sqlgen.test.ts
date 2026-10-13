import assert from "node:assert/strict";
import { test } from "node:test";
import { compile, qIdent, qStr } from "../src/engine/sqlgen.ts";
import { EngineError } from "../src/engine/contract.ts";
import type { PlanGraph, PlanNode } from "../src/engine/contract.ts";

const schema = {
  fields: [{ name: "id", data_type: "int64" }, { name: "category", data_type: "utf8" }, { name: "geometry", data_type: { other: "geometry" } }],
  geometry_columns: [{ name: "geometry", geometry_type: "polygon", crs: { epsg: 4326 } }],
};
const bbox = { min_x: 104, min_y: 12, max_x: 105, max_y: 13, crs: { epsg: 4326 } as const };
const cov = { xmin: "bbox.xmin", ymin: "bbox.ymin", xmax: "bbox.xmax", ymax: "bbox.ymax", declared: true };
const src: PlanNode = { op: "source", uri: "a.parquet", format: "geo_parquet", schema };
const sources = { "a.parquet": { url: "/api/files/a", covering: cov } };

function plan(nodes: Record<string, PlanNode>, root: string): PlanGraph {
  return { ir_version: "0.2", nodes, root };
}

test("quoting", () => {
  assert.equal(qIdent('a"b'), '"a""b"');
  assert.equal(qStr("it's"), "'it''s'");
});

test("bbox filter uses the covering columns; count aggregate; one CTE per node", () => {
  const p = plan(
    {
      s: src,
      f: { op: "bbox_filter", input: "s", geometry_column: "geometry", bbox },
      a: { op: "aggregate", input: "f", group_by: [], aggregates: [{ func: "count", column: null, alias: "n" }] },
    },
    "a",
  );
  const { sql, files } = compile(p, sources, 1000);
  assert.match(sql, /read_parquet\('gf_src_0\.parquet'\)/);
  assert.match(sql, /"bbox"\."xmin" <= 105\.0 AND "bbox"\."xmax" >= 104\.0 AND "bbox"\."ymin" <= 13\.0 AND "bbox"\."ymax" >= 12\.0/);
  assert.match(sql, /COUNT\(\*\) AS "n"/);
  assert.match(sql, /LIMIT 1001$/);
  assert.equal(files.length, 1);
  assert.equal(files[0].url, "/api/files/a");
});

test("a shared source is compiled once", () => {
  const p = plan(
    {
      s: src,
      a: { op: "filter", input: "s", predicate: { kind: "compare", column: "id", op: "le", value: { int: 2 } } },
      b: { op: "filter", input: "s", predicate: { kind: "compare", column: "id", op: "ge", value: { int: 2 } } },
      // not a real join (unsupported) — use two consumers feeding limit/sort instead
      l: { op: "limit", input: "a", n: 1 },
    },
    "l",
  );
  void p.nodes.b; // b is an orphan in this tiny plan; only the shared-source behaviour is under test
  const { sql } = compile(p, sources, 10);
  assert.equal(sql.match(/read_parquet\(/g)?.length, 1);
});

test("literals are escaped (no injection through values or identifiers)", () => {
  const p = plan(
    {
      s: src,
      f: { op: "filter", input: "s", predicate: { kind: "compare", column: 'cat"; DROP', op: "eq", value: { str: "x' OR '1'='1" } } },
    },
    "f",
  );
  const { sql } = compile(p, sources, 10);
  assert.ok(sql.includes(`"cat""; DROP" = 'x'' OR ''1''=''1'`), sql);
});

test("unsupported operations and missing covering are rejected, not approximated", () => {
  const spatial = plan(
    { s: src, i: { op: "intersects", input: "s", geometry_column: "geometry", with: { wkt: "POINT(0 0)", crs: { epsg: 4326 } } } },
    "i",
  );
  assert.throws(() => compile(spatial, sources, 10), (e: unknown) => e instanceof EngineError && e.kind === "unsupported");
  const noCov = plan({ s: src, f: { op: "bbox_filter", input: "s", geometry_column: "geometry", bbox } }, "f");
  assert.throws(() => compile(noCov, { "a.parquet": { url: "/x", covering: null } }, 10), (e: unknown) => e instanceof EngineError && e.kind === "unsupported");
});

test("structural problems: cycle, missing node, unbound source, non-finite number", () => {
  assert.throws(() => compile(plan({ a: { op: "limit", input: "b", n: 1 }, b: { op: "limit", input: "a", n: 1 } }, "a"), sources, 10), /cycle/);
  assert.throws(() => compile(plan({ a: { op: "limit", input: "ghost", n: 1 } }, "a"), sources, 10), /missing node/);
  assert.throws(() => compile(plan({ s: src }, "s"), {}, 10), /not bound/);
  const nan = plan({ s: src, f: { op: "bbox_filter", input: "s", geometry_column: "geometry", bbox: { ...bbox, min_x: Number.NaN } } }, "f");
  assert.throws(() => compile(nan, sources, 10), /non-finite/);
});
