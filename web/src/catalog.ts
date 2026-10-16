export interface CatalogFile { path: string; size_bytes: number; rows?: number | null }
export interface CatalogDataset {
  id: string;
  title: { vi: string; en: string };
  group: string;
  provider: string;
  license: { name: string; url?: string | null; attribution?: string | null; redistribution?: string | null };
  coverage: { name: string; bbox: [number, number, number, number] };
  crs: string;
  geometry_type: string;
  format_normalized: string;
  source_ids: string[];
  present: boolean;
  files: CatalogFile[];
  /** COG files of a raster dataset (the gateway returns a plain list), null for vector data */
  raster: { name: string; url: string; size_bytes: number }[] | null;
  quality: { rows?: number; null_geometry_count?: number; invalid_geometry_count?: number | null } | null;
  notes?: string | null;
}
export interface Catalog {
  datasets: CatalogDataset[];
  attributions: string[];
  errors: string[];
}
export const GROUP_ORDER = ["base", "buildings", "transport", "admin", "places", "raster", "thematic", "benchmark"] as const;

export function groupKey(group: string): string {
  return `catalog.group.${group}`;
}
export function sourceIdOf(dataset: CatalogDataset): string | null {
  return dataset.source_ids[0] ?? null;
}
export function datasetsByGroup(catalog: Catalog, opts: { developer: boolean }) {
  return GROUP_ORDER.flatMap((group) => {
    if (group === "benchmark" && !opts.developer) return [];
    const datasets = catalog.datasets.filter((d) => d.group === group && (opts.developer || d.present));
    return datasets.length ? [{ group, datasets }] : [];
  });
}
/** Credits to show: the basemap's, plus the attribution of the datasets on screen (all present datasets when none is given). */
export function attributionText(catalog: Catalog, presentIdsOnScreen?: Iterable<string>): string {
  const visible = presentIdsOnScreen ? new Set(presentIdsOnScreen) : null;
  const text = [
    "© OpenFreeMap © OpenMapTiles Data from OpenStreetMap",
    ...catalog.datasets
      .filter((d) => d.present && (!visible || visible.has(d.id)))
      .map((d) => d.license.attribution)
      .filter((x): x is string => Boolean(x)),
  ];
  return [...new Set(text.map((x) => x.trim()).filter(Boolean))].join(" · ");
}
export async function fetchCatalog(): Promise<Catalog | null> {
  try {
    const r = await fetch("/api/catalog");
    if (!r.ok) return null;
    const value = await r.json() as Catalog;
    return Array.isArray(value.datasets) ? value : null;
  } catch {
    return null;
  }
}
