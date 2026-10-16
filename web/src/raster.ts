/**
 * Terrain (Copernicus DEM GLO-30, Cloud Optimized GeoTIFF) as a map overlay.
 *
 * The browser reads the COG itself, with HTTP range requests against the gateway (`/api/rasters/<id>/<file>`), and only the
 * internal OVERVIEW it needs (a 1-degree tile is 3600 x 3600 px; its 900 x 900 overview is read in a few requests). Elevation is
 * drawn as a colour ramp with hillshade into a canvas that MapLibre shows as an image source. This is display, not analysis:
 * raster operations are not part of the PlanGraph yet (see the report: architecture deviation / open decision).
 *
 * The pure helpers (tile names, ramp, hillshade) are unit-tested without a browser.
 */
import { fromUrl } from "geotiff";

export type BBox = [number, number, number, number]; // west, south, east, north (degrees)

/** `Copernicus_DSM_COG_10_N10_00_E106_00_DEM.tif` -> [106, 10, 107, 11]; null for any other name. Tiles are named by their SW corner. */
export function tileBBox(fileName: string): BBox | null {
  const m = /_([NS])(\d{2})_00_([EW])(\d{3})_00_DEM/.exec(fileName);
  if (!m) return null;
  const lat = (m[1] === "N" ? 1 : -1) * Number(m[2]);
  const lon = (m[3] === "E" ? 1 : -1) * Number(m[4]);
  return [lon, lat, lon + 1, lat + 1];
}

export function unionBBox(boxes: BBox[]): BBox {
  return boxes.reduce<BBox>((u, b) => [Math.min(u[0], b[0]), Math.min(u[1], b[1]), Math.max(u[2], b[2]), Math.max(u[3], b[3])], [Infinity, Infinity, -Infinity, -Infinity]);
}

/** Anchor stops of the hypsometric ramp: [fraction of the (min..max) range, r, g, b]. Sea level sits at the dark-blue / green edge. */
const RAMP: [number, number, number, number][] = [
  [0.0, 34, 62, 110], [0.08, 52, 102, 145], [0.12, 84, 140, 96], [0.35, 142, 178, 98],
  [0.6, 214, 200, 120], [0.8, 170, 120, 80], [1.0, 245, 240, 235],
];

/** Colour of an elevation: values below `min` clamp to the first stop, above `max` to the last. */
export function elevationColor(value: number, min: number, max: number): [number, number, number] {
  const span = max - min;
  const f = span > 0 ? Math.min(1, Math.max(0, (value - min) / span)) : 0;
  for (let i = 1; i < RAMP.length; i++) {
    if (f <= RAMP[i][0]) {
      const [f0, r0, g0, b0] = RAMP[i - 1];
      const [f1, r1, g1, b1] = RAMP[i];
      const k = f1 > f0 ? (f - f0) / (f1 - f0) : 0;
      return [Math.round(r0 + (r1 - r0) * k), Math.round(g0 + (g1 - g0) * k), Math.round(b0 + (b1 - b0) * k)];
    }
  }
  const last = RAMP[RAMP.length - 1];
  return [last[1], last[2], last[3]];
}

export interface Hillshade { azimuthDeg?: number; altitudeDeg?: number; zFactor?: number }

/**
 * Horn hillshade, 0 (dark) .. 1 (lit). `cellX`/`cellY` are the pixel sizes in metres. Border pixels reuse their neighbour's slope
 * (no data is invented beyond the tile). Flat ground lights to sin(altitude), so a flat plain is mid-grey rather than black.
 */
export function hillshade(values: ArrayLike<number>, width: number, height: number, cellX: number, cellY: number, o: Hillshade = {}): Float32Array {
  const az = ((o.azimuthDeg ?? 315) * Math.PI) / 180, alt = ((o.altitudeDeg ?? 45) * Math.PI) / 180, z = o.zFactor ?? 2;
  const out = new Float32Array(width * height);
  const at = (x: number, y: number) => values[Math.min(height - 1, Math.max(0, y)) * width + Math.min(width - 1, Math.max(0, x))];
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const a = at(x - 1, y - 1), b = at(x, y - 1), c = at(x + 1, y - 1), d = at(x - 1, y), f = at(x + 1, y), g = at(x - 1, y + 1), h = at(x, y + 1), i = at(x + 1, y + 1);
      const dzdx = ((c + 2 * f + i) - (a + 2 * d + g)) / (8 * cellX);
      const dzdy = ((g + 2 * h + i) - (a + 2 * b + c)) / (8 * cellY);
      const slope = Math.atan(z * Math.hypot(dzdx, dzdy));
      const aspect = Math.atan2(dzdy, -dzdx);
      const lit = Math.sin(alt) * Math.cos(slope) + Math.cos(alt) * Math.sin(slope) * Math.cos(az - aspect);
      out[y * width + x] = Math.min(1, Math.max(0, lit));
    }
  }
  return out;
}

export interface DemOverlay {
  canvas: HTMLCanvasElement;
  /** west, south, east, north of the whole mosaic */
  bbox: BBox;
  min: number;
  max: number;
  tiles: number;
  /** size of one tile in pixels at the overview that was read */
  tilePx: number;
  /** how many bytes of the COGs were actually requested (range requests), for the metadata line */
  bytesRead: number;
}

export interface DemFile { name: string; url: string; size_bytes?: number }

/** Pick the smallest internal image that is still at least `minPx` wide (index 0 is full resolution, the last the coarsest). */
export function pickOverview(widths: number[], minPx: number): number {
  let best = 0;
  widths.forEach((w, i) => { if (w >= minPx && w <= widths[best]) best = i; else if (widths[best] < minPx && w > widths[best]) best = i; });
  return best;
}

/** Read the tiles (range requests, overview only), mosaic them and paint ramp + hillshade. Throws on network or decode errors. */
export async function loadDem(files: DemFile[], opts: { minPx?: number; signal?: AbortSignal } = {}): Promise<DemOverlay> {
  const placed = files.map((f) => ({ f, box: tileBBox(f.name) })).filter((x): x is { f: DemFile; box: BBox } => x.box !== null);
  if (!placed.length) throw new Error("no DEM tile with a recognisable name");
  const bbox = unionBBox(placed.map((p) => p.box));
  const reads: { box: BBox; data: Float32Array; w: number; h: number }[] = [];
  let tilePx = 0;
  let bytesRead = 0;
  for (const { f, box } of placed) {
    const tiff = await fromUrl(f.url, { allowFullFile: false }, opts.signal);
    const count = await tiff.getImageCount();
    const widths: number[] = [];
    for (let i = 0; i < count; i++) widths.push((await tiff.getImage(i)).getWidth());
    const image = await tiff.getImage(pickOverview(widths, opts.minPx ?? 900));
    const w = image.getWidth(), h = image.getHeight();
    const raster = (await image.readRasters({ interleave: false, signal: opts.signal })) as unknown as ArrayLike<number>[];
    reads.push({ box, data: Float32Array.from(raster[0]), w, h });
    tilePx = w;
    bytesRead += w * h * 4; // decoded size: the transferred (compressed) size is smaller; this is an upper bound of the payload
  }
  let min = Infinity, max = -Infinity;
  for (const r of reads) for (const v of r.data) if (Number.isFinite(v) && v > -500 && v < 9000) { if (v < min) min = v; if (v > max) max = v; }
  if (!Number.isFinite(min)) throw new Error("DEM tiles contain no valid elevation");
  const cols = Math.round(bbox[2] - bbox[0]), rows = Math.round(bbox[3] - bbox[1]);
  const canvas = document.createElement("canvas");
  canvas.width = cols * tilePx;
  canvas.height = rows * tilePx;
  const ctx = canvas.getContext("2d");
  if (!ctx) throw new Error("no 2d canvas");
  for (const r of reads) {
    const lat = (r.box[1] + r.box[3]) / 2;
    const cellY = (111_320 * 1) / r.h, cellX = (111_320 * Math.cos((lat * Math.PI) / 180)) / r.w;
    const shade = hillshade(r.data, r.w, r.h, cellX, cellY);
    const image = ctx.createImageData(r.w, r.h);
    for (let i = 0; i < r.data.length; i++) {
      const [cr, cg, cb] = elevationColor(r.data[i], min, max);
      const s = 0.35 + 0.65 * shade[i];
      image.data[i * 4] = Math.round(cr * s);
      image.data[i * 4 + 1] = Math.round(cg * s);
      image.data[i * 4 + 2] = Math.round(cb * s);
      image.data[i * 4 + 3] = 255;
    }
    ctx.putImageData(image, Math.round((r.box[0] - bbox[0]) * tilePx), Math.round((bbox[3] - r.box[3]) * tilePx));
  }
  return { canvas, bbox, min, max, tiles: reads.length, tilePx, bytesRead };
}
