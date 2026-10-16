import { test } from "node:test";
import assert from "node:assert/strict";
import { elevationColor, hillshade, pickOverview, tileBBox, unionBBox } from "../src/raster.ts";

test("tile names give the 1-degree box of their south-west corner", () => {
  assert.deepEqual(tileBBox("Copernicus_DSM_COG_10_N10_00_E106_00_DEM.tif"), [106, 10, 107, 11]);
  assert.deepEqual(tileBBox("Copernicus_DSM_COG_10_S01_00_W002_00_DEM.tif"), [-2, -1, -1, 0]);
  assert.equal(tileBBox("index.json"), null);
});

test("the four HCMC tiles form one 2 x 2 degree mosaic", () => {
  const boxes = ["N10_00_E106_00", "N10_00_E107_00", "N11_00_E106_00", "N11_00_E107_00"].map((t) => tileBBox(`Copernicus_DSM_COG_10_${t}_DEM.tif`)!);
  assert.deepEqual(unionBBox(boxes), [106, 10, 108, 12]);
});

test("overview choice: smallest image that is still wide enough; full resolution when nothing is", () => {
  assert.equal(pickOverview([3600, 1800, 900, 450], 900), 2);
  assert.equal(pickOverview([3600, 1800, 900, 450], 400), 3);
  assert.equal(pickOverview([3600, 1800, 900, 450], 5000), 0);
  assert.equal(pickOverview([3600], 900), 0);
});

test("elevation ramp: clamps, is continuous, and rises from blue (sea) to white (summit)", () => {
  const lo = elevationColor(-100, 0, 100), hi = elevationColor(500, 0, 100);
  assert.deepEqual(lo, elevationColor(0, 0, 100));
  assert.deepEqual(hi, elevationColor(100, 0, 100));
  assert.ok(lo[2] > lo[0], "lowest is blue");
  assert.ok(hi[0] > 200 && hi[1] > 200 && hi[2] > 200, "highest is near white");
  let prev = elevationColor(0, 0, 100);
  for (let v = 1; v <= 100; v++) {
    const c = elevationColor(v, 0, 100);
    assert.ok(Math.abs(c[0] - prev[0]) <= 40 && Math.abs(c[1] - prev[1]) <= 40 && Math.abs(c[2] - prev[2]) <= 40, `jump at ${v}`);
    prev = c;
  }
  assert.deepEqual(elevationColor(5, 5, 5), elevationColor(0, 0, 0), "a flat range does not divide by zero");
});

test("hillshade: flat ground is mid-lit, slopes facing the sun are brighter than slopes facing away", () => {
  const w = 9, h = 9;
  const flat = new Float32Array(w * h).fill(10);
  const flatShade = hillshade(flat, w, h, 30, 30);
  assert.ok(Math.abs(flatShade[4 * w + 4] - Math.sin((45 * Math.PI) / 180)) < 1e-6);
  // a ramp rising to the east: the sun is at azimuth 315 (north-west), so west-facing... check both directions
  const eastUp = new Float32Array(w * h), westUp = new Float32Array(w * h);
  for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) { eastUp[y * w + x] = x * 10; westUp[y * w + x] = (w - 1 - x) * 10; }
  const a = hillshade(eastUp, w, h, 30, 30)[4 * w + 4];
  const b = hillshade(westUp, w, h, 30, 30)[4 * w + 4];
  assert.ok(b > a, `slope rising to the west faces the north-west sun: ${b} > ${a}`);
});
