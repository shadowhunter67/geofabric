import assert from "node:assert/strict";
import { test } from "node:test";
import { fitTarget, startView, VIETNAM_SANE_BOUNDS, VIETNAM_START_BOUNDS, visibleBounds, zoomToFit, type BBox } from "../src/vietnam.ts";

const within = (inner: BBox, outer: BBox) => inner[0] >= outer[0] && inner[1] >= outer[1] && inner[2] <= outer[2] && inner[3] <= outer[3];
// map areas of the desktop layouts the UI is checked at (width minus the left panel, height minus top bar and bottom panel)
const VIEWPORTS: [number, number][] = [[1050, 330], [1120, 420], [1350, 415], [1600, 500]];

test("the reviewed start view contains the whole mainland and sits inside the sanity box", () => {
  const mainland: BBox = [102.14, 8.18, 109.67, 23.39]; // the extent of Overture's Vietnam administrative polygons (measured, data/local/overture_admin_hcm.parquet)
  assert.ok(within(mainland, VIETNAM_START_BOUNDS));
  assert.ok(within(VIETNAM_START_BOUNDS, VIETNAM_SANE_BOUNDS));
});

test("startup camera: the whole of Vietnam is visible and the fit is tight on every desktop viewport (never a Southeast-Asia-scale frame)", () => {
  for (const [w, h] of VIEWPORTS) {
    const v = visibleBounds(VIETNAM_START_BOUNDS, w, h, 12);
    assert.ok(within(VIETNAM_START_BOUNDS, v), `Vietnam is cut off at ${w}x${h}: sees ${v.map((x) => x.toFixed(1))}`);
    const latSpan = v[3] - v[1], startLat = VIETNAM_START_BOUNDS[3] - VIETNAM_START_BOUNDS[1];
    // wide windows show neighbours sideways (Vietnam is tall and narrow); the vertical frame must still be Vietnam, not a whole region
    assert.ok(latSpan <= startLat * 1.25, `frame ${latSpan.toFixed(1)} deg tall for a ${startLat.toFixed(1)} deg country at ${w}x${h}`);
    assert.ok(v[1] > 0 && v[3] < 30, `latitude window ${v[1].toFixed(1)}..${v[3].toFixed(1)} leaves Vietnam's part of the world`);
  }
});

test("regression: the old hard-coded camera (centre 105,14 at zoom 5) cut Vietnam off and showed ~30 degrees of longitude", () => {
  const w = 1350, h = 415, worldPx = 512 * 2 ** 5;
  const lonSpan = (w / worldPx) * 360;
  assert.ok(lonSpan > 25, `the old view was ${lonSpan.toFixed(0)} deg wide`);
  const needed = zoomToFit(VIETNAM_START_BOUNDS, w, h, 12);
  assert.ok(needed < 5, `Vietnam only fits below zoom 5 (needs ${needed.toFixed(2)}): the fixed zoom 5 could never show the whole country in this viewport`);
});

test("fit-to-layer: real extents pass through unchanged", () => {
  const hcmc: BBox = [106.66, 10.74, 106.74, 10.81];
  const national: BBox = [102.14, 8.18, 109.67, 23.39];
  assert.deepEqual(fitTarget(hcmc), { bounds: hcmc, adjusted: false });
  assert.deepEqual(fitTarget(national), { bounds: national, adjusted: false });
});

test("fit-to-layer: a known outlier can never become an absurd camera target", () => {
  const cases: [string, readonly number[] | null | undefined][] = [
    ["Southeast Asia-wide extent from one bad polygon", [90, -10, 130, 30]],
    ["world-spanning polygon", [-180, -90, 180, 90]],
    ["partly outside Vietnam", [99, 8, 105, 12]],
    ["inverted", [107, 11, 106, 10]],
    ["NaN", [Number.NaN, 10, 106, 11]],
    ["null", null],
    ["short", [1, 2, 3]],
  ];
  for (const [name, b] of cases) {
    const t = fitTarget(b);
    assert.equal(t.adjusted, true, name);
    assert.deepEqual(t.bounds, VIETNAM_START_BOUNDS, name);
    assert.ok(t.reason, name);
  }
});

test("regression: the opening camera is always finite, even when the map container has no size yet (MapLibre's `bounds` option produced NaN there)", () => {
  for (const [w, h] of [[0, 0], [0, 400], [900, 0], [-5, -5], [Number.NaN, Number.NaN], [1350, 415]] as [number, number][]) {
    const v = startView(w, h);
    assert.ok(Number.isFinite(v.center[0]) && Number.isFinite(v.center[1]) && Number.isFinite(v.zoom), `${w}x${h}`);
    assert.ok(v.zoom > 2 && v.zoom < 8, `${w}x${h}: zoom ${v.zoom}`);
    assert.ok(v.center[0] > VIETNAM_START_BOUNDS[0] && v.center[0] < VIETNAM_START_BOUNDS[2] && v.center[1] > VIETNAM_START_BOUNDS[1] && v.center[1] < VIETNAM_START_BOUNDS[3]);
  }
});
