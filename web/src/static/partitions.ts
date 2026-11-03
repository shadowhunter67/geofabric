/**
 * Physical partitioning of a logical dataset (ADR-017): a dataset is one logical layer for the user and a list of immutable GeoParquet shards
 * on disk. This module is pure (no DOM, no network): it validates a dataset's partition manifest and answers "which shards does this view need".
 *
 * Shard ids never appear in a query: the plan keeps naming the logical dataset (LogicalDataset -> PhysicalSource[], ADR-013), so sharding is
 * not an IR change.
 */
import { VIETNAM_SANE_BOUNDS, type BBox } from "../vietnam.ts";

export const PARTITION_MANIFEST_SCHEMA = 3;
export const ALLOWED_CRS = ["OGC:CRS84"] as const;
export const MAX_SHARD_BYTES = 50_000_000;
export const QUADTREE_ROOT: BBox = [96, 0, 128, 32];
export const QUADTREE_MAX_DEPTH = 13;

const CELL_ID = /^[qL][0-3]{0,13}$/;

/** The CRS84 rectangle for an adaptive-quadtree-v1 cell. Digits are SW, SE, NW, NE. */
export function cellBBox(id: string): BBox {
  if (!CELL_ID.test(id)) throw new Error(`invalid quadtree cell id ${id}`);
  let [west, south, east, north] = QUADTREE_ROOT;
  for (const digit of id.slice(1)) {
    const middleX = (west + east) / 2;
    const middleY = (south + north) / 2;
    if (digit === "0" || digit === "2") east = middleX; else west = middleX;
    if (digit === "0" || digit === "1") north = middleY; else south = middleY;
  }
  return [west, south, east, north];
}

/** The depth-specific q cell containing a point. Midpoint ties go east/north, matching the Python builder. */
export function cellIdForPoint(x: number, y: number, depth: number): string {
  if (!Number.isInteger(depth) || depth < 0 || depth > QUADTREE_MAX_DEPTH) throw new Error(`depth must be an integer between 0 and ${QUADTREE_MAX_DEPTH}`);
  if (!Number.isFinite(x) || !Number.isFinite(y)) throw new Error("point coordinates must be finite");
  let [west, south, east, north] = QUADTREE_ROOT;
  if (x < west || x > east || y < south || y > north) throw new Error(`point (${x}, ${y}) lies outside the quadtree root`);
  let id = "q";
  for (let i = 0; i < depth; i++) {
    const middleX = (west + east) / 2;
    const middleY = (south + north) / 2;
    const eastHalf = x >= middleX;
    const northHalf = y >= middleY;
    id += String(Number(eastHalf) + 2 * Number(northHalf));
    if (eastHalf) west = middleX; else east = middleX;
    if (northHalf) south = middleY; else north = middleY;
  }
  return id;
}

/** One immutable shard. `bbox` is the DATA extent of the shard (union of its features' boxes), so a feature that straddles a cell border is still found. */
export interface PartitionEntry {
  id: string;
  bbox: BBox;
  url: string;
  bytes: number;
  sha256: string;
  format: "GeoParquet";
  crs: string;
  rows: number;
  license: string;
  source_release: string;
  geometry_type?: string;
  kind?: "regular" | "oversize";
  min_zoom?: number | null;
  max_zoom?: number | null;
  /** rows per square degree of the shard's data extent (informational) */
  density?: number | null;
}

/** What one dataset's manifest says. Lives at its own URL; the root manifest lists these. */
export interface DatasetPartitions {
  schema: typeof PARTITION_MANIFEST_SCHEMA;
  dataset: string;
  data_version: string;
  /** physical layout, versioned independently of the data (a re-partition never silently changes old URLs) */
  partition_scheme: string;
  /** the logical layer is only read at or above this map zoom (country and regional views never open its shards) */
  min_zoom: number;
  rows: number;
  bbox: BBox;
  partitions: PartitionEntry[];
}

export interface RootManifest {
  schema: typeof PARTITION_MANIFEST_SCHEMA;
  data_version: string;
  partition_scheme: string;
  built_with: { code_commit: string; source_release: string };
  datasets: { id: string; manifest_url: string; bytes: number; sha256: string; partitions: number; rows: number; bbox: BBox }[];
}

const HEX64 = /^[0-9a-f]{64}$/;
const isBox = (b: unknown): b is BBox => Array.isArray(b) && b.length === 4 && b.every((v) => typeof v === "number" && Number.isFinite(v)) && b[0] < b[2] && b[1] < b[3];
const intersects = (a: BBox, b: BBox) => a[0] <= b[2] && a[2] >= b[0] && a[1] <= b[3] && a[3] >= b[1];
const finiteOptional = (value: number | null | undefined) => value == null || Number.isFinite(value);

/** Every problem found in a dataset manifest (empty = valid). Deterministic and cheap enough for pull-request CI. */
export function validateDatasetPartitions(m: DatasetPartitions, opts: { stage?: "build" | "published" } = {}): string[] {
  const out: string[] = [];
  const stage = opts.stage ?? "published";
  if (m.schema !== PARTITION_MANIFEST_SCHEMA) out.push(`schema ${m.schema} is not ${PARTITION_MANIFEST_SCHEMA}`);
  if (!m.dataset) out.push("dataset id is empty");
  if (!m.data_version || /latest/i.test(m.data_version)) out.push("data_version must be an immutable version, never 'latest'");
  if (!m.partition_scheme) out.push("partition_scheme is empty");
  if (!(m.min_zoom >= 0 && m.min_zoom <= 22)) out.push(`min_zoom ${m.min_zoom} is not a map zoom`);
  if (!isBox(m.bbox)) out.push("dataset bbox is not a valid box");
  const seen = new Set<string>();
  let rows = 0;
  let union: BBox | undefined;
  for (const p of m.partitions) {
    const at = `partition ${p.id}`;
    if (!p.id) out.push("a partition has no id");
    else if (!CELL_ID.test(p.id)) out.push(`${at}: id must match ^[qL][0-3]{0,13}$`);
    else if (seen.has(p.id)) out.push(`duplicate partition id ${p.id}`);
    seen.add(p.id);
    if (!isBox(p.bbox)) out.push(`${at}: bbox is not a valid box`);
    else {
      const [sx0, sy0, sx1, sy1] = VIETNAM_SANE_BOUNDS;
      if (p.bbox[0] < sx0 || p.bbox[1] < sy0 || p.bbox[2] > sx1 || p.bbox[3] > sy1) out.push(`${at}: bbox leaves the Vietnam sanity box (outlier)`);
      if (isBox(m.bbox) && !(p.bbox[0] >= m.bbox[0] - 1e-9 && p.bbox[1] >= m.bbox[1] - 1e-9 && p.bbox[2] <= m.bbox[2] + 1e-9 && p.bbox[3] <= m.bbox[3] + 1e-9)) out.push(`${at}: bbox is outside the dataset bbox`);
      if (CELL_ID.test(p.id) && !intersects(p.bbox, cellBBox(p.id))) out.push(`${at}: bbox does not intersect its quadtree cell`);
      union = union
        ? [Math.min(union[0], p.bbox[0]), Math.min(union[1], p.bbox[1]), Math.max(union[2], p.bbox[2]), Math.max(union[3], p.bbox[3])]
        : [...p.bbox];
    }
    if (stage === "build") {
      if (p.url !== `${m.dataset}/${p.id}.parquet`) out.push(`${at}: build url must be exactly ${m.dataset}/${p.id}.parquet`);
    } else if (!/^https:\/\//.test(p.url) || /\/latest\//.test(p.url) || /@latest|@main|@master/.test(p.url)) out.push(`${at}: url must be an https URL pinned to a commit or a content hash`);
    if (!HEX64.test(p.sha256)) out.push(`${at}: sha256 is not 64 lowercase hex characters`);
    if (!Number.isFinite(p.bytes) || !(p.bytes > 0)) out.push(`${at}: bytes must be positive and finite`);
    else if (p.bytes > MAX_SHARD_BYTES) out.push(`${at}: bytes exceed the ${MAX_SHARD_BYTES} byte shard limit`);
    if (!Number.isFinite(p.rows) || !(p.rows > 0)) out.push(`${at}: rows must be positive and finite`);
    if (Number.isFinite(p.rows) && Number.isFinite(p.bytes) && p.rows > p.bytes) out.push(`${at}: rows cannot exceed bytes`);
    if (!finiteOptional(p.density)) out.push(`${at}: density must be finite when present`);
    if (!finiteOptional(p.min_zoom)) out.push(`${at}: min_zoom must be finite when present`);
    if (!finiteOptional(p.max_zoom)) out.push(`${at}: max_zoom must be finite when present`);
    if (p.min_zoom != null && Number.isFinite(p.min_zoom) && (p.min_zoom < 0 || p.min_zoom > 22)) out.push(`${at}: min_zoom ${p.min_zoom} is not a map zoom`);
    if (p.max_zoom != null && Number.isFinite(p.max_zoom) && (p.max_zoom < 0 || p.max_zoom > 22)) out.push(`${at}: max_zoom ${p.max_zoom} is not a map zoom`);
    if (p.min_zoom != null && p.max_zoom != null && Number.isFinite(p.min_zoom) && Number.isFinite(p.max_zoom) && p.min_zoom > p.max_zoom) out.push(`${at}: min_zoom cannot exceed max_zoom`);
    if (p.kind != null && p.kind !== (p.id.startsWith("L") ? "oversize" : "regular")) out.push(`${at}: kind ${p.kind} disagrees with its id prefix`);
    if (p.format !== "GeoParquet") out.push(`${at}: format ${p.format} is not GeoParquet`);
    if (!(ALLOWED_CRS as readonly string[]).includes(p.crs)) out.push(`${at}: crs ${p.crs} is not an allowed CRS`);
    if (!p.license) out.push(`${at}: license is empty`);
    if (!p.source_release) out.push(`${at}: source_release is empty`);
    rows += p.rows;
  }
  if (rows !== m.rows) out.push(`partition rows add up to ${rows}, the dataset says ${m.rows}`);
  if (isBox(m.bbox) && union && m.bbox.some((value, i) => Math.abs(value - union![i]) > 1e-9)) out.push("dataset bbox does not equal the union of partition bboxes");
  return out;
}

export interface PartitionStats {
  count: number;
  regular: number;
  oversize: number;
  bytes: { min: number; median: number; p90: number; max: number; total: number };
  rows: number;
}

/** Compact physical-layout diagnostics. Empty manifests report zero for every byte statistic. */
export function partitionStats(m: DatasetPartitions): PartitionStats {
  const bytes = m.partitions.map((p) => p.bytes).sort((a, b) => a - b);
  const count = bytes.length;
  const middle = Math.floor(count / 2);
  const median = count === 0 ? 0 : count % 2 ? bytes[middle] : (bytes[middle - 1] + bytes[middle]) / 2;
  return {
    count,
    regular: m.partitions.filter((p) => p.id.startsWith("q")).length,
    oversize: m.partitions.filter((p) => p.id.startsWith("L")).length,
    bytes: {
      min: count ? bytes[0] : 0,
      median,
      p90: count ? bytes[Math.ceil(count * 0.9) - 1] : 0,
      max: count ? bytes[count - 1] : 0,
      total: bytes.reduce((sum, value) => sum + value, 0),
    },
    rows: m.partitions.reduce((sum, p) => sum + p.rows, 0),
  };
}

/** A uniform grid over the shard boxes: a view looks at a handful of grid cells instead of every shard. */
export interface PartitionIndex {
  cell: number;
  cells: Map<number, number[]>;
  partitions: PartitionEntry[];
}

const CELL = 0.5; // degrees; shard boxes are 0.05-1 degrees, so each lands in a few cells
const key = (ix: number, iy: number) => ix * 100003 + iy;

export function buildIndex(partitions: readonly PartitionEntry[]): PartitionIndex {
  const sorted = [...partitions].sort((a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0)); // deterministic order whatever the manifest order
  const cells = new Map<number, number[]>();
  sorted.forEach((p, i) => {
    for (let ix = Math.floor(p.bbox[0] / CELL); ix <= Math.floor(p.bbox[2] / CELL); ix++)
      for (let iy = Math.floor(p.bbox[1] / CELL); iy <= Math.floor(p.bbox[3] / CELL); iy++) {
        const k = key(ix, iy);
        const l = cells.get(k);
        if (l) l.push(i); else cells.set(k, [i]);
      }
  });
  return { cell: CELL, cells, partitions: sorted };
}

/** The shards whose data extent meets the view, in id order. Physical planning only: it never changes what a query means. */
export function shardsInView(index: PartitionIndex, view: BBox): PartitionEntry[] {
  const hit = new Set<number>();
  for (let ix = Math.floor(view[0] / index.cell); ix <= Math.floor(view[2] / index.cell); ix++)
    for (let iy = Math.floor(view[1] / index.cell); iy <= Math.floor(view[3] / index.cell); iy++)
      for (const i of index.cells.get(key(ix, iy)) ?? []) if (intersects(index.partitions[i].bbox, view)) hit.add(i);
  return [...hit].sort((a, b) => a - b).map((i) => index.partitions[i]);
}

export interface ViewSelection {
  /** shards to read (empty when the layer is gated by zoom or nothing is there) */
  shards: PartitionEntry[];
  /** why nothing is read: the layer needs a closer zoom */
  gated: boolean;
  /** more shards than the fetch budget allows: the caller reads `shards` (the first `limit`) and shows the layer as partial, never silently complete */
  truncated: boolean;
}

/** Zoom policy first (a country view never opens building shards), then the physical lookup, bounded by `limit`. */
export function selectForView(index: PartitionIndex, minZoom: number, view: BBox, zoom: number, limit = 24): ViewSelection {
  if (zoom < minZoom) return { shards: [], gated: true, truncated: false };
  const all = shardsInView(index, view);
  if (all.length <= limit) return { shards: all, gated: false, truncated: false };
  // over budget: keep the shards nearest the centre of the view (the part the user is looking at), id as the tie-break so the choice is deterministic
  const cx = (view[0] + view[2]) / 2, cy = (view[1] + view[3]) / 2;
  const d = (p: PartitionEntry) => Math.hypot((p.bbox[0] + p.bbox[2]) / 2 - cx, (p.bbox[1] + p.bbox[3]) / 2 - cy);
  const near = [...all].sort((a, b) => d(a) - d(b) || (a.id < b.id ? -1 : 1)).slice(0, limit);
  return { shards: near, gated: false, truncated: true };
}
