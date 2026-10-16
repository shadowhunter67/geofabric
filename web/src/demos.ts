/**
 * Demo workflows over the Vietnam pack. Each one is a description (which datasets to show, where to look, which gateway query
 * templates to run); the queries go through the same `/api/query` -> PlanGraph -> planner -> executor path as every other query.
 * Nothing here talks to a data file or computes a spatial result: the frontend only asks and shows.
 * Pure data and helpers, unit-tested.
 */
import type { Key } from "./ui/i18n.ts";

export type DemoId = "a" | "b" | "c" | "d" | "e";

export interface DemoSpec {
  id: DemoId;
  title: Key;
  desc: Key;
  /** catalog dataset ids shown while the demo runs (the others are switched off) */
  layers: string[];
  /** where to look: lon/lat centre and MapLibre zoom (a viewport at which the query is meaningful and fast) */
  view: { center: [number, number]; zoom: number };
}

export const BUILDINGS = "overture_buildings_vn_south";
export const ROADS = "overture_transport_hcm";
export const ADMIN = "overture_admin_hcm";
export const PLACES = "overture_places_hcm";
export const DEM = "copernicus_dem_hcm";

/** Road classes that count as "main roads" in the demo (a residential street is not). */
export const MAIN_ROAD_CLASSES = ["motorway", "trunk", "primary"];
/** Distance of demo B, in metres. The gateway turns it into an explicit angular distance and says so in the response. */
export const NEAR_DISTANCE_M = 200;
/** Schools and hospitals: the real `category` values of the Overture places in the pack (see the data quality notes). */
export const SCHOOL_CATEGORIES = ["elementary_school", "middle_school", "high_school", "preschool", "college_university"];
export const HOSPITAL_CATEGORIES = ["hospital"];

export const DEMOS: DemoSpec[] = [
  { id: "a", title: "demo.a.title", desc: "demo.a.desc", layers: [], view: { center: [106.6985, 10.7765], zoom: 15.2 } },
  { id: "b", title: "demo.b.title", desc: "demo.b.desc", layers: [ROADS], view: { center: [106.70, 10.78], zoom: 14.4 } },
  { id: "c", title: "demo.c.title", desc: "demo.c.desc", layers: [ADMIN], view: { center: [106.70, 10.78], zoom: 13.4 } },
  { id: "d", title: "demo.d.title", desc: "demo.d.desc", layers: [ADMIN], view: { center: [106.70, 10.78], zoom: 14.2 } },
  { id: "e", title: "demo.e.title", desc: "demo.e.desc", layers: [DEM], view: { center: [107.0, 10.95], zoom: 9.2 } },
];

/** Gateway request bodies of the demos (the viewport bbox is added by the caller). */
export function demoRequests(id: DemoId): Record<string, unknown>[] {
  switch (id) {
    case "a":
      return [{ source: BUILDINGS, kind: "features", limit: 5000 }];
    case "b":
      return [
        { source: BUILDINGS, kind: "near", limit: 5000, with: { source: ROADS, filter: { column: "class", op: "in", values: MAIN_ROAD_CLASSES }, distance_m: NEAR_DISTANCE_M } },
        { source: BUILDINGS, kind: "near_count", with: { source: ROADS, filter: { column: "class", op: "in", values: MAIN_ROAD_CLASSES }, distance_m: NEAR_DISTANCE_M } },
      ];
    case "c":
      return [{ source: BUILDINGS, kind: "join_count_by", with: { source: ADMIN, filter: { column: "subtype", op: "eq", value: "locality" }, group_by: "name" } }];
    case "d":
      return [
        { source: PLACES, kind: "features", limit: 5000, filter: { column: "category", op: "in", values: [...SCHOOL_CATEGORIES, ...HOSPITAL_CATEGORIES] } },
        { source: PLACES, kind: "count_by", group_by: "category", filter: { column: "category", op: "in", values: [...SCHOOL_CATEGORIES, ...HOSPITAL_CATEGORIES] } },
      ];
    case "e":
      return [];
  }
}

/** Split the `count_by category` rows of demo D into schools and hospitals. */
export function splitPlaces(rows: { category?: unknown; count?: unknown }[]): { schools: number; hospitals: number } {
  let schools = 0, hospitals = 0;
  for (const r of rows) {
    const n = Number(r.count ?? 0);
    const c = String(r.category ?? "");
    if (HOSPITAL_CATEGORIES.includes(c)) hospitals += n;
    else if (SCHOOL_CATEGORIES.includes(c)) schools += n;
  }
  return { schools, hospitals };
}

/** The ward with the most buildings in demo C (rows are sorted by the gateway; this does not rely on it). */
export function topGroup(rows: { count?: unknown }[], key: string): { name: string; count: number } | null {
  let best: { name: string; count: number } | null = null;
  for (const r of rows) {
    const count = Number(r.count ?? 0);
    const name = String((r as Record<string, unknown>)[key] ?? "");
    if (!best || count > best.count) best = { name, count };
  }
  return best;
}
