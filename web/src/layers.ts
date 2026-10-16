import type { ExpressionSpecification, LayerSpecification } from "maplibre-gl";
import type { CatalogDataset } from "./catalog.ts";

export interface LayerState { visible: boolean; opacity: number }
export const LAYER_STORAGE_KEY = "gf.layers";

export const COLOURS = {
  buildingsFill: "#4aa3ff", buildingsLine: "#8cc4ff", admin: "#d7a8ff",
  motorway: "#f06d4f", primary: "#f3a43b", secondary: "#f1d460",
  tertiary: "#d8e0e8", local: "#b8c1c9", places: "#77d7d1",
} as const;

/** Administrative layer: provinces, wards, neighbourhoods on land. Country outlines duplicate the basemap; territorial-water rows duplicate units. */
const ADMIN_LAND: ExpressionSpecification = ["all", ["!=", ["get", "subtype"], "country"], ["!=", ["get", "is_land"], false]];

export function loadLayerStates(): Record<string, LayerState> {
  try {
    const parsed = JSON.parse(localStorage.getItem(LAYER_STORAGE_KEY) ?? "{}") as Record<string, LayerState>;
    return parsed && typeof parsed === "object" ? parsed : {};
  } catch { return {}; }
}
export function saveLayerStates(states: Record<string, LayerState>): void {
  try { localStorage.setItem(LAYER_STORAGE_KEY, JSON.stringify(states)); } catch { /* storage unavailable */ }
}
export function renderPolicy(group: string, zoom: number): { minZoom: number; limit: number; hint: "zoom_in" | null } {
  const p = ({
    // A layer shows the first N rows of the viewport in file order, so N must exceed what a viewport at minZoom holds
    // (measured on the HCMC pack: ~2,500 places and ~9,000 main-class road segments at these zooms), else the map looks cut off.
    buildings: [11, 5000], transport: [10, 15000], places: [14, 6000],
    admin: [0, 1000], thematic: [10, 3000],
  } as Record<string, [number, number]>)[group] ?? [99, 0];
  return { minZoom: p[0], limit: p[1], hint: zoom < p[0] ? "zoom_in" : null };
}
/** Road classes worth drawing at a zoom (a dense city has ~13,000 main-class segments in a district-sized view). */
export function classFilter(zoom: number): string[] | null {
  if (zoom < 12) return ["motorway", "trunk", "primary"];
  if (zoom < 14) return ["motorway", "trunk", "primary", "secondary", "tertiary"];
  return null;
}
export function applyOpacity(spec: LayerSpecification, opacity: number): LayerSpecification {
  const paint = { ...(spec.paint ?? {}) } as Record<string, unknown>;
  const key = spec.type === "fill" ? "fill-opacity" : spec.type === "line" ? "line-opacity" : spec.type === "circle" ? "circle-opacity" : null;
  if (key) {
    const existing = paint[key];
    paint[key] = typeof existing === "number" ? existing * opacity : opacity;
  }
  return { ...spec, paint } as LayerSpecification;
}
export function layerSpecs(dataset: CatalogDataset, sourceId: string, opacity = 1): LayerSpecification[] {
  const id = `ctx-${dataset.id}`;
  const source = sourceId;
  const specs: LayerSpecification[] = dataset.group === "buildings" ? [
    { id: `${id}-fill`, type: "fill", source, filter: ["==", ["geometry-type"], "Polygon"], paint: { "fill-color": COLOURS.buildingsFill, "fill-opacity": .5 } },
    { id: `${id}-line`, type: "line", source, paint: { "line-color": COLOURS.buildingsLine, "line-width": 1 } },
  ] : dataset.group === "transport" ? [
    { id: `${id}-line`, type: "line", source, layout: { "line-sort-key": ["match", ["get", "class"], "motorway", 8, "trunk", 7, "primary", 6, "secondary", 5, "tertiary", 4, 1] }, paint: {
      "line-color": ["match", ["get", "class"], "motorway", COLOURS.motorway, "trunk", COLOURS.motorway, "primary", COLOURS.primary, "secondary", COLOURS.secondary, "tertiary", COLOURS.tertiary, "residential", COLOURS.local, "service", COLOURS.local, COLOURS.local],
      "line-width": ["interpolate", ["linear"], ["zoom"], 10, 1, 14, 4, 18, 10],
      "line-dasharray": ["match", ["get", "class"], "footway", ["literal", [2, 2]], "path", ["literal", [2, 2]], "track", ["literal", [2, 2]], ["literal", [1, 0]]],
    } },
  ] : dataset.group === "admin" ? [
    { id: `${id}-fill`, type: "fill", source, filter: ADMIN_LAND, paint: { "fill-color": COLOURS.admin, "fill-opacity": 0.01 } },
    { id: `${id}-line`, type: "line", source, filter: ADMIN_LAND, paint: { "line-color": COLOURS.admin, "line-width": ["match", ["get", "subtype"], "region", 1.5, 1], "line-dasharray": ["match", ["get", "subtype"], "microhood", ["literal", [2, 2]], ["literal", [1, 0]]] } },
    { id: `${id}-label`, type: "symbol", source, filter: ADMIN_LAND, layout: { "text-field": ["get", "name"], "text-font": ["Noto Sans Regular"], "text-size": 12 }, paint: { "text-color": "#f2eaff", "text-halo-color": "#1a2026", "text-halo-width": 1 } },
  ] : dataset.group === "places" ? [
    { id: `${id}-circle`, type: "circle", source, paint: { "circle-radius": ["interpolate", ["linear"], ["zoom"], 12, 3, 16, 6], "circle-color": ["match", ["get", "category"], ["elementary_school", "middle_school", "high_school", "preschool", "college_university", "specialty_school"], "#76b7eb", ["hospital", "dental_clinic", "primary_care_or_general_clinic", "pediatric_clinic"], "#ef7185", ["government_office", "government_department", "community_and_government"], "#d7a8ff", ["restaurant", "coffee_shop", "cafe", "casual_eatery", "food_and_beverage_store"], "#f3a43b", ["shopping", "fashion_and_apparel_store", "electronics_store", "hardware_home_and_garden_store"], "#df8fd0", ["parking"], "#f1d460", ["park", "national_park", "amusement_park"], "#74be83", COLOURS.places], "circle-stroke-color": "#fff", "circle-stroke-width": .5 } },
  ] : [
    { id: `${id}-fill`, type: "fill", source, paint: { "fill-color": ["match", ["get", "class"], "park", "#5ba86a", "garden", "#70b77a", "forest", "#3e8751", "residential", "#7185a3", "commercial", "#a27a90", "industrial", "#8e7a64", "#899198"], "fill-opacity": .35 } },
  ];
  return specs.map((s) => applyOpacity(s, opacity));
}
