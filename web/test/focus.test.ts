import { test } from "node:test";
import assert from "node:assert/strict";
import { extentOf, geometryBBox, planFocus, pxAtZoom, zoomForPx } from "../src/focus.ts";

const square = (cx: number, cy: number, s: number): GeoJSON.Feature => ({
  type: "Feature", properties: {},
  geometry: { type: "Polygon", coordinates: [[[cx, cy], [cx + s, cy], [cx + s, cy + s], [cx, cy + s], [cx, cy]]] },
});
const fc = (features: GeoJSON.Feature[]): GeoJSON.FeatureCollection => ({ type: "FeatureCollection", features });

test("bbox of polygons, multipolygons and points", () => {
  assert.deepEqual(geometryBBox(square(1, 2, 3).geometry), [1, 2, 4, 5]);
  assert.deepEqual(geometryBBox({ type: "Point", coordinates: [7, 8] }), [7, 8, 7, 8]);
  assert.deepEqual(geometryBBox({ type: "MultiPolygon", coordinates: [[[[0, 0], [1, 0], [1, 1], [0, 0]]], [[[5, 5], [6, 5], [6, 6], [5, 5]]]] }), [0, 0, 6, 6]);
  assert.equal(geometryBBox(null), null);
});

test("zoom and pixel size are inverse", () => {
  assert.ok(Math.abs(pxAtZoom(0.01, zoomForPx(0.01, 12)) - 12) < 1e-9);
});

test("tiny buildings spread over a whole region are not visible: the plan zooms to the densest area", () => {
  // 200 buildings in a dense cluster near (105.5, 10.5), 40 scattered over 10 degrees
  const features: GeoJSON.Feature[] = [];
  for (let i = 0; i < 200; i++) features.push(square(105.5 + (i % 20) * 0.0008, 10.5 + Math.floor(i / 20) * 0.0008, 0.0006));
  for (let i = 0; i < 40; i++) features.push(square(100 + (i * 0.25) % 10, 8 + ((i * 0.37) % 8), 0.0006));
  const f = planFocus(fc(features), { zoom: 5 });
  assert.ok(f, "must plan a focus");
  assert.ok(Math.abs(f!.center[0] - 105.5) < 0.05 && Math.abs(f!.center[1] - 10.5) < 0.05, `centre ${f!.center}`);
  assert.ok(f!.zoom > 10, `zoom ${f!.zoom}`);
  assert.ok(pxAtZoom(0.0006 * Math.SQRT2, f!.zoom) >= 8, "a typical feature is several pixels wide at the planned zoom");
});

test("features that are already visible need no focus", () => {
  const features = Array.from({ length: 20 }, (_, i) => square(i * 0.01, 0, 0.005));
  assert.equal(planFocus(fc(features), { zoom: 13 }), null);
});

test("points and near-empty results need no focus; extent helper", () => {
  const pts = fc(Array.from({ length: 10 }, (_, i) => ({ type: "Feature" as const, properties: {}, geometry: { type: "Point" as const, coordinates: [i, i] } })));
  assert.equal(planFocus(pts, { zoom: 4 }), null);
  assert.equal(planFocus(fc([square(0, 0, 1)]), { zoom: 4 }), null);
  assert.deepEqual(extentOf(fc([square(0, 0, 1), square(5, 5, 1)])), [0, 0, 6, 6]);
  assert.equal(extentOf(fc([])), null);
});
