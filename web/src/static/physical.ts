import type { PlanGraph } from "../engine/contract.ts";
import type { ShardBinding } from "../engine/contract.ts";
import { buildIndex, selectForView, validateDatasetPartitions } from "./partitions.ts";
import type { DatasetPartitions, PartitionEntry, PartitionIndex } from "./partitions.ts";
import type { BBox } from "../vietnam.ts";

const RANGE_MIN = 1024 * 1024;
const DEFAULT_MAX_SHARDS = 24;
const PREFIX_OPS = new Set(["filter", "project", "bbox_filter"]);

type RuntimeNode = Record<string, unknown> & { op: string };

function degreeBBox(node: RuntimeNode): BBox | null {
  if (node.op !== "bbox_filter") return null;
  const bbox = node.bbox as { min_x?: unknown; min_y?: unknown; max_x?: unknown; max_y?: unknown; crs?: unknown } | undefined;
  const crs = bbox?.crs;
  const degreeCrs = crs === "ogc_crs84" || (
    typeof crs === "object" && crs !== null && "epsg" in crs && (crs as { epsg?: unknown }).epsg === 4326
  );
  const values = [bbox?.min_x, bbox?.min_y, bbox?.max_x, bbox?.max_y];
  if (!degreeCrs || !values.every((value) => typeof value === "number" && Number.isFinite(value))) return null;
  return values as BBox;
}

function intersection(boxes: readonly BBox[]): BBox | null {
  if (boxes.length === 0) return null;
  const west = Math.max(...boxes.map((box) => box[0]));
  const south = Math.max(...boxes.map((box) => box[1]));
  const east = Math.min(...boxes.map((box) => box[2]));
  const north = Math.min(...boxes.map((box) => box[3]));
  if (west > east || south > north) return [west, south, west, south];
  return [west, south, east, north];
}

function union(a: BBox, b: BBox): BBox {
  return [Math.min(a[0], b[0]), Math.min(a[1], b[1]), Math.max(a[2], b[2]), Math.max(a[3], b[3])];
}

/** The degree extent each logical source must make available to the plan, or null when it is unbounded. */
export function requiredExtents(plan: PlanGraph): Record<string, BBox | null> {
  const nodes = plan.nodes as Record<string, RuntimeNode>;
  const consumers = new Map<string, string[]>();
  for (const [id, node] of Object.entries(nodes)) {
    for (const key of ["input", "left", "right"] as const) {
      const dependency = node[key];
      if (typeof dependency !== "string") continue;
      const list = consumers.get(dependency);
      if (list) list.push(id); else consumers.set(dependency, [id]);
    }
  }
  for (const list of consumers.values()) list.sort();

  const result: Record<string, BBox | null> = {};
  for (const [sourceId, source] of Object.entries(nodes)) {
    if (source.op !== "source" || typeof source.uri !== "string") continue;
    const chains: Array<BBox | null> = [];

    const walk = (id: string, boxes: readonly BBox[], path: ReadonlySet<string>) => {
      const next = consumers.get(id) ?? [];
      if (next.length === 0) {
        chains.push(intersection(boxes));
        return;
      }
      for (const consumerId of next) {
        if (path.has(consumerId)) {
          chains.push(null);
          continue;
        }
        const consumer = nodes[consumerId];
        if (!consumer || !PREFIX_OPS.has(consumer.op)) {
          chains.push(intersection(boxes));
          continue;
        }
        let nextBoxes = boxes;
        if (consumer.op === "bbox_filter") {
          const box = degreeBBox(consumer);
          if (!box) {
            chains.push(null);
            continue;
          }
          nextBoxes = [...boxes, box];
        }
        walk(consumerId, nextBoxes, new Set([...path, consumerId]));
      }
    };

    walk(sourceId, [], new Set([sourceId]));
    const extent = chains.some((box) => box === null)
      ? null
      : (chains as BBox[]).reduce<BBox | null>((combined, box) => combined ? union(combined, box) : [...box], null);
    const existing = result[source.uri];
    if (!(source.uri in result)) result[source.uri] = extent;
    else if (existing === null || extent === null) result[source.uri] = null;
    else result[source.uri] = union(existing, extent);
  }
  return result;
}

/** Whole-file access for small shards or hosts without byte-range support. */
export function shardAccess(bytes: number, rangeHost: boolean): "full" | "range" {
  return rangeHost && bytes > RANGE_MIN ? "range" : "full";
}

export interface LoadedPartitions { manifest: DatasetPartitions; index: PartitionIndex }

function abortError(): DOMException {
  return new DOMException("The operation was aborted", "AbortError");
}

function forCaller<T>(shared: Promise<T>, signal?: AbortSignal): Promise<T> {
  if (!signal) return shared;
  if (signal.aborted) return Promise.reject(abortError());
  return new Promise<T>((resolve, reject) => {
    const aborted = () => reject(abortError());
    signal.addEventListener("abort", aborted, { once: true });
    shared.then(resolve, reject).finally(() => signal.removeEventListener("abort", aborted));
  });
}

/** Registry and one-fetch cache for published partition manifests. */
export class PartitionCatalog {
  private readonly registrations = new Map<string, { manifest_url: string }>();
  private readonly loaded = new Map<string, LoadedPartitions>();
  private readonly pending = new Map<string, Promise<LoadedPartitions>>();
  private readonly fetchJson: (url: string, signal?: AbortSignal) => Promise<unknown>;

  constructor(fetchJson: (url: string, signal?: AbortSignal) => Promise<unknown>) {
    this.fetchJson = fetchJson;
  }

  register(datasetId: string, registration: { manifest_url: string }): void {
    this.registrations.set(datasetId, registration);
    this.loaded.delete(datasetId);
    this.pending.delete(datasetId);
  }

  has(id: string): boolean {
    return this.registrations.has(id);
  }

  load(id: string, signal?: AbortSignal): Promise<LoadedPartitions> {
    if (signal?.aborted) return Promise.reject(abortError());
    const registration = this.registrations.get(id);
    if (!registration) return Promise.reject(new Error(`unknown partitioned dataset: ${id}`));
    const cached = this.loaded.get(id);
    if (cached) return forCaller(Promise.resolve(cached), signal);

    let shared = this.pending.get(id);
    if (!shared) {
      shared = Promise.resolve()
        .then(() => this.fetchJson(registration.manifest_url))
        .then((raw) => {
          if (typeof raw !== "object" || raw === null || !Array.isArray((raw as { partitions?: unknown }).partitions)) {
            throw new Error("invalid partition manifest: expected an object with a partitions array");
          }
          const manifest = raw as DatasetPartitions;
          const problems = validateDatasetPartitions(manifest, { stage: "published" });
          if (problems.length) throw new Error(`invalid partition manifest for ${id}: ${problems.slice(0, 5).join("; ")}`);
          const loaded = { manifest, index: buildIndex(manifest.partitions) };
          this.loaded.set(id, loaded);
          return loaded;
        });
      this.pending.set(id, shared);
      shared.then(
        () => { if (this.pending.get(id) === shared) this.pending.delete(id); },
        () => { if (this.pending.get(id) === shared) this.pending.delete(id); },
      );
    }
    return forCaller(shared, signal);
  }
}

export interface ResolvedSource {
  uri: string;
  dataset: string;
  state: "ok" | "empty" | "gated" | "truncated";
  shards: PartitionEntry[];
  estimatedBytes: number;
  extent: BBox | null;
  minZoom: number;
  probe?: PartitionEntry;
  reason?: string;
}

function totalBytes(shards: readonly PartitionEntry[]): number {
  return shards.reduce((sum, shard) => sum + shard.bytes, 0);
}

function smallestShard(shards: readonly PartitionEntry[]): PartitionEntry | undefined {
  return [...shards].sort((a, b) => a.bytes - b.bytes || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0))[0];
}

/** Resolve only the partitioned logical sources named by `datasets`. */
export async function resolveSources(
  plan: PlanGraph,
  datasets: Record<string, string>,
  catalog: PartitionCatalog,
  view: { zoom: number | null },
  opts: { maxShards?: number } = {},
  signal?: AbortSignal,
): Promise<Record<string, ResolvedSource>> {
  const extents = requiredExtents(plan);
  const maxShards = opts.maxShards ?? DEFAULT_MAX_SHARDS;
  const entries = await Promise.all(Object.entries(datasets).filter(([uri]) => uri in extents).map(async ([uri, dataset]) => {
    const { manifest, index } = await catalog.load(dataset, signal);
    const extent = extents[uri];
    const base = { uri, dataset, extent, minZoom: manifest.min_zoom };
    if (typeof view.zoom === "number" && view.zoom < manifest.min_zoom) {
      return [uri, { ...base, state: "gated", shards: [], estimatedBytes: 0, reason: `needs zoom ${manifest.min_zoom} or closer` }] as const;
    }
    if (extent === null) {
      if (index.partitions.length > maxShards) {
        return [uri, { ...base, state: "gated", shards: [], estimatedBytes: 0, reason: `needs the whole dataset (${index.partitions.length} shards)` }] as const;
      }
      const shards = [...index.partitions];
      return [uri, { ...base, state: "ok", shards, estimatedBytes: totalBytes(shards) }] as const;
    }
    const selected = selectForView(index, manifest.min_zoom, extent, view.zoom ?? manifest.min_zoom, maxShards);
    if (selected.truncated) {
      return [uri, { ...base, state: "truncated", shards: selected.shards, estimatedBytes: totalBytes(selected.shards), reason: `more than ${maxShards} shards intersect the extent` }] as const;
    }
    if (selected.shards.length === 0) {
      const probe = smallestShard(index.partitions);
      return [uri, { ...base, state: "empty", shards: [], estimatedBytes: probe?.bytes ?? 0, ...(probe ? { probe } : {}) }] as const;
    }
    return [uri, { ...base, state: "ok", shards: selected.shards, estimatedBytes: totalBytes(selected.shards) }] as const;
  }));
  return Object.fromEntries(entries);
}

/** Convert a resolved source into the shard fields accepted by SourceBinding. */
export function toBindings(resolved: ResolvedSource, rangeHost: boolean): { url: string; access: "full" | "range"; shards: ShardBinding[] } {
  const physical = resolved.shards.length ? resolved.shards : resolved.probe ? [resolved.probe] : [];
  if (physical.length === 0) throw new Error(`resolved source ${resolved.uri} has no shards to bind (${resolved.state})`);
  const shards = physical.map((shard) => ({ id: shard.id, url: shard.url, bytes: shard.bytes, access: shardAccess(shard.bytes, rangeHost) }));
  return { url: shards[0].url, access: shards[0].access, shards };
}
