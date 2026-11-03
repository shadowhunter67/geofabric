/**
 * Physical partitioning of a logical dataset (ADR-017): a dataset is one logical layer for the user and a list of immutable GeoParquet shards
 * on disk. This module is pure (no DOM, no network): it validates a dataset's partition manifest and answers "which shards does this view need".
 *
 * Shard ids never appear in a query: the plan keeps naming the logical dataset (LogicalDataset -> PhysicalSource[], ADR-013), so sharding is
 * not an IR change.
 */
import { VIETNAM_SANE_BOUNDS, type BBox } from "../vietnam.ts";

export const PARTITION_MANIFEST_SCHEMA = 3;

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
  /** rows per square degree of the shard's data extent (informational) */
  density?: number;
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

/** Every problem found in a dataset manifest (empty = valid). Deterministic and cheap enough for pull-request CI. */
export function validateDatasetPartitions(m: DatasetPartitions): string[] {
  const out: string[] = [];
  if (m.schema !== PARTITION_MANIFEST_SCHEMA) out.push(`schema ${m.schema} is not ${PARTITION_MANIFEST_SCHEMA}`);
  if (!m.dataset) out.push("dataset id is empty");
  if (!m.data_version || /latest/i.test(m.data_version)) out.push("data_version must be an immutable version, never 'latest'");
  if (!m.partition_scheme) out.push("partition_scheme is empty");
  if (!(m.min_zoom >= 0 && m.min_zoom <= 22)) out.push(`min_zoom ${m.min_zoom} is not a map zoom`);
  if (!isBox(m.bbox)) out.push("dataset bbox is not a valid box");
  const seen = new Set<string>();
  let rows = 0;
  for (const p of m.partitions) {
    const at = `partition ${p.id}`;
    if (!p.id) out.push("a partition has no id");
    else if (seen.has(p.id)) out.push(`duplicate partition id ${p.id}`);
    seen.add(p.id);
    if (!isBox(p.bbox)) { out.push(`${at}: bbox is not a valid box`); continue; }
    const [sx0, sy0, sx1, sy1] = VIETNAM_SANE_BOUNDS;
    if (p.bbox[0] < sx0 || p.bbox[1] < sy0 || p.bbox[2] > sx1 || p.bbox[3] > sy1) out.push(`${at}: bbox leaves the Vietnam sanity box (outlier)`);
    if (isBox(m.bbox) && !(p.bbox[0] >= m.bbox[0] - 1e-9 && p.bbox[1] >= m.bbox[1] - 1e-9 && p.bbox[2] <= m.bbox[2] + 1e-9 && p.bbox[3] <= m.bbox[3] + 1e-9)) out.push(`${at}: bbox is outside the dataset bbox`);
    if (!/^https:\/\//.test(p.url) || /\/latest\//.test(p.url) || /@latest|@main|@master/.test(p.url)) out.push(`${at}: url must be an https URL pinned to a commit or a content hash`);
    if (!HEX64.test(p.sha256)) out.push(`${at}: sha256 is not 64 lowercase hex characters`);
    if (!(p.bytes > 0)) out.push(`${at}: bytes must be positive`);
    if (!(p.rows > 0)) out.push(`${at}: rows must be positive`);
    if (p.format !== "GeoParquet") out.push(`${at}: format ${p.format} is not GeoParquet`);
    if (!p.crs) out.push(`${at}: crs is empty`);
    if (!p.license) out.push(`${at}: license is empty`);
    if (!p.source_release) out.push(`${at}: source_release is empty`);
    rows += p.rows;
  }
  if (rows !== m.rows) out.push(`partition rows add up to ${rows}, the dataset says ${m.rows}`);
  return out;
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
