/**
 * Vietnam-wide geography the app commits to: the reviewed first view and the sanity box that keeps a bad extent from ever becoming a camera.
 * Pure functions, no map, no DOM.
 */

export type BBox = [number, number, number, number]; // [west, south, east, north], degrees

/**
 * The first view of the public demo: mainland Vietnam (about 102.1-109.5 E, 8.5-23.4 N) with a sea margin. The offshore archipelagos are
 * reachable by panning but are deliberately not part of the opening frame: including them would shrink the mainland to a sliver.
 */
export const VIETNAM_START_BOUNDS: BBox = [101.8, 8.0, 110.2, 23.9];

/**
 * Everything that can legitimately be "Vietnam and its sea", archipelagos included. A dataset extent that leaves this box is an outlier
 * (a bad geometry, a wrong CRS, a world-spanning polygon), never a camera target.
 */
export const VIETNAM_SANE_BOUNDS: BBox = [100, 6, 118, 25];

export interface FitTarget { bounds: BBox; adjusted: boolean; reason?: string }

const finite4 = (b: readonly number[] | null | undefined): b is BBox => !!b && b.length === 4 && b.every((v) => Number.isFinite(v));

/**
 * Where "go to this layer" is allowed to look. A declared extent inside the sanity box is used as it is; anything else (not finite,
 * inverted, partly or wholly outside Vietnam) falls back to the reviewed start view and says why. It never repairs a geometry.
 */
export function fitTarget(b: readonly number[] | null | undefined): FitTarget {
  if (!finite4(b)) return { bounds: VIETNAM_START_BOUNDS, adjusted: true, reason: "extent is missing or not finite" };
  if (!(b[0] < b[2] && b[1] < b[3])) return { bounds: VIETNAM_START_BOUNDS, adjusted: true, reason: "extent is empty or inverted" };
  const [sx0, sy0, sx1, sy1] = VIETNAM_SANE_BOUNDS;
  if (b[0] < sx0 || b[1] < sy0 || b[2] > sx1 || b[3] > sy1) return { bounds: VIETNAM_START_BOUNDS, adjusted: true, reason: "extent leaves the Vietnam sanity box (outlier)" };
  return { bounds: [b[0], b[1], b[2], b[3]], adjusted: false };
}

/** Web Mercator y (in units of the world width) of a latitude. */
const mercY = (lat: number): number => 0.5 - Math.log(Math.tan(Math.PI / 4 + (Math.max(-85, Math.min(85, lat)) * Math.PI) / 360)) / (2 * Math.PI);

/** The MapLibre zoom (512-px tiles) at which `b` fits a `w` x `h` pixel viewport with `pad` pixels on every side. */
export function zoomToFit(b: BBox, w: number, h: number, pad = 0): number {
  const dx = (b[2] - b[0]) / 360;
  const dy = mercY(b[1]) - mercY(b[3]);
  const zx = Math.log2(Math.max(1, w - 2 * pad) / (512 * dx));
  const zy = Math.log2(Math.max(1, h - 2 * pad) / (512 * dy));
  return Math.min(zx, zy);
}

/** The geographic window a `w` x `h` viewport shows when centred on `b` at the zoom that fits it (same projection as above). */
export function visibleBounds(b: BBox, w: number, h: number, pad = 0): BBox {
  const z = zoomToFit(b, w, h, pad);
  const worldPx = 512 * 2 ** z;
  const cx = (b[0] + b[2]) / 2, cyM = (mercY(b[1]) + mercY(b[3])) / 2;
  const halfLon = (w / 2 / worldPx) * 360;
  const yTop = cyM - h / 2 / worldPx, yBot = cyM + h / 2 / worldPx;
  const lat = (y: number) => (Math.atan(Math.sinh(Math.PI * (1 - 2 * y))) * 180) / Math.PI;
  return [cx - halfLon, lat(yBot), cx + halfLon, lat(yTop)];
}

export interface StartView { center: [number, number]; zoom: number }

/**
 * The opening camera as a plain centre and zoom. It is computed here (not by MapLibre's `bounds` option): a map container that has no size yet
 * (0 x 0 at construction) makes `bounds` fit to a NaN camera that poisons every later move. A missing or zero size falls back to a typical
 * desktop map area.
 */
export function startView(w: number, h: number): StartView {
  const width = w > 0 ? w : 1200, height = h > 0 ? h : 500;
  const z = zoomToFit(VIETNAM_START_BOUNDS, width, height, 12);
  return {
    center: [(VIETNAM_START_BOUNDS[0] + VIETNAM_START_BOUNDS[2]) / 2, (VIETNAM_START_BOUNDS[1] + VIETNAM_START_BOUNDS[3]) / 2],
    zoom: Number.isFinite(z) ? Math.max(0, Math.min(z, 8)) : 4.5,
  };
}
