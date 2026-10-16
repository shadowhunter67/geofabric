/**
 * Choosing a sensible first view of a result.
 *
 * A result is a set of real feature geometries. Fitted to a whole dataset (hundreds of km), a building of 150 m is far below one pixel,
 * so the viewer sees nothing of it. `planFocus` finds out whether the features would be visible at the current zoom and, if not, picks
 * the densest area and a zoom at which a typical feature is a few pixels wide. Pure functions: no map, no DOM.
 */

export type BBox = [number, number, number, number];

function walk(c: unknown, f: (x: number, y: number) => void): void {
  if (Array.isArray(c) && typeof c[0] === "number") f(c[0] as number, c[1] as number);
  else if (Array.isArray(c)) for (const k of c) walk(k, f);
}

export function geometryBBox(g: GeoJSON.Geometry | null | undefined): BBox | null {
  if (!g) return null;
  let b: BBox | null = null;
  const coords = g.type === "GeometryCollection" ? g.geometries.map((x) => ("coordinates" in x ? x.coordinates : [])) : [(g as { coordinates: unknown }).coordinates];
  walk(coords, (x, y) => {
    if (!b) b = [x, y, x, y];
    else { if (x < b[0]) b[0] = x; if (y < b[1]) b[1] = y; if (x > b[2]) b[2] = x; if (y > b[3]) b[3] = y; }
  });
  return b;
}

export function extentOf(fc: GeoJSON.FeatureCollection): BBox | null {
  let out: BBox | null = null;
  for (const f of fc.features) {
    const b = geometryBBox(f.geometry);
    if (!b) continue;
    if (!out) out = [...b] as BBox;
    else { out[0] = Math.min(out[0], b[0]); out[1] = Math.min(out[1], b[1]); out[2] = Math.max(out[2], b[2]); out[3] = Math.max(out[3], b[3]); }
  }
  return out;
}

/** Size in pixels of `deg` degrees at a MapLibre zoom (512-px tiles). */
export const pxAtZoom = (deg: number, zoom: number): number => (deg * 512 * 2 ** zoom) / 360;
/** The zoom at which `deg` degrees span `px` pixels. */
export const zoomForPx = (deg: number, px: number): number => Math.log2((px * 360) / (512 * deg));

export interface Focus { center: [number, number]; zoom: number; medianPx: number; features: number }

export interface FocusOptions {
  zoom: number;        // current map zoom
  minPx?: number;      // a typical feature smaller than this is "not visible" (default 4)
  targetPx?: number;   // zoom to this size (default 12)
  maxZoom?: number;
  grid?: number;       // density grid size (default 16)
}

/** null when the features are already visible at the current zoom (or there is nothing to focus on). */
export function planFocus(fc: GeoJSON.FeatureCollection, o: FocusOptions): Focus | null {
  const minPx = o.minPx ?? 4, targetPx = o.targetPx ?? 12, maxZoom = o.maxZoom ?? 17, G = o.grid ?? 16;
  const items: { cx: number; cy: number; diag: number }[] = [];
  for (const f of fc.features) {
    const b = geometryBBox(f.geometry);
    if (!b) continue;
    items.push({ cx: (b[0] + b[2]) / 2, cy: (b[1] + b[3]) / 2, diag: Math.hypot(b[2] - b[0], b[3] - b[1]) });
  }
  if (items.length < 2) return null;
  const diags = items.map((i) => i.diag).sort((a, b) => a - b);
  const median = diags[Math.floor(diags.length / 2)];
  if (!(median > 0)) return null; // points: always visible as dots
  const medianPx = pxAtZoom(median, o.zoom);
  if (medianPx >= minPx) return null;
  // densest cell of the centroids
  let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
  for (const i of items) { x0 = Math.min(x0, i.cx); y0 = Math.min(y0, i.cy); x1 = Math.max(x1, i.cx); y1 = Math.max(y1, i.cy); }
  const w = Math.max(x1 - x0, 1e-9), h = Math.max(y1 - y0, 1e-9);
  const cells = new Map<number, { n: number; sx: number; sy: number }>();
  for (const i of items) {
    const gx = Math.min(G - 1, Math.floor(((i.cx - x0) / w) * G)), gy = Math.min(G - 1, Math.floor(((i.cy - y0) / h) * G));
    const k = gy * G + gx;
    const c = cells.get(k) ?? { n: 0, sx: 0, sy: 0 };
    c.n++; c.sx += i.cx; c.sy += i.cy; cells.set(k, c);
  }
  let best = { n: 0, sx: 0, sy: 0 };
  for (const c of cells.values()) if (c.n > best.n) best = c;
  const zoom = Math.min(maxZoom, Math.max(o.zoom, zoomForPx(median, targetPx)));
  return { center: [best.sx / best.n, best.sy / best.n], zoom, medianPx, features: items.length };
}
