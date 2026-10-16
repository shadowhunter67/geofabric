import { test } from "node:test";
import assert from "node:assert/strict";
import { ADMIN, BUILDINGS, DEMOS, MAIN_ROAD_CLASSES, NEAR_DISTANCE_M, ROADS, demoRequests, splitPlaces, topGroup } from "../src/demos.ts";

test("five demos, each with a title, a description and a view", () => {
  assert.deepEqual(DEMOS.map((d) => d.id), ["a", "b", "c", "d", "e"]);
  for (const d of DEMOS) assert.ok(d.view.zoom > 0 && d.view.center.length === 2);
});

test("every demo query is a gateway template over catalog sources (never a data file or raw SQL)", () => {
  for (const d of DEMOS) {
    for (const r of demoRequests(d.id)) {
      assert.match(String(r.source), /^overture_/);
      assert.ok(["features", "count_by", "near", "near_count", "join_count_by"].includes(String(r.kind)), String(r.kind));
      assert.equal("sql" in r, false);
    }
  }
});

test("demo B asks for main roads within 200 m, once for the map and once for the distinct count", () => {
  const [near, count] = demoRequests("b");
  assert.equal(near.kind, "near");
  assert.equal(count.kind, "near_count");
  for (const r of [near, count]) {
    const w = r.with as { source: string; distance_m: number; filter: { column: string; op: string; values: string[] } };
    assert.equal(w.source, ROADS);
    assert.equal(w.distance_m, NEAR_DISTANCE_M);
    assert.deepEqual(w.filter, { column: "class", op: "in", values: MAIN_ROAD_CLASSES });
  }
  assert.equal(near.source, BUILDINGS);
});

test("demo C joins buildings with wards (locality) and groups by name", () => {
  const [c] = demoRequests("c");
  assert.equal(c.kind, "join_count_by");
  assert.deepEqual(c.with, { source: ADMIN, filter: { column: "subtype", op: "eq", value: "locality" }, group_by: "name" });
});

test("demo E is display only: no query at all", () => {
  assert.deepEqual(demoRequests("e"), []);
});

test("splitPlaces separates schools from hospitals and ignores other categories", () => {
  assert.deepEqual(splitPlaces([
    { category: "elementary_school", count: 10 }, { category: "high_school", count: 4 }, { category: "hospital", count: 3 },
    { category: "restaurant", count: 99 }, { category: "preschool", count: "5" },
  ]), { schools: 19, hospitals: 3 });
  assert.deepEqual(splitPlaces([]), { schools: 0, hospitals: 0 });
});

test("topGroup finds the largest group whatever the order", () => {
  assert.deepEqual(topGroup([{ count: 2, name: "x" }, { count: 9, name: "y" }, { count: 5, name: "z" }] as never, "name"), { name: "y", count: 9 });
  assert.equal(topGroup([], "name"), null);
});
