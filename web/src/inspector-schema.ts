import { t } from "./ui/i18n.ts";
import type { Lang } from "./ui/i18n.ts";
import { dictionary, getLang } from "./ui/i18n.ts";
import { fmtInt } from "./ui/labels.ts";

const has = (key: string): boolean => key in dictionary(getLang());

export interface InspectorRow { label: string; value: string; raw?: string }
export interface InspectorContext { lang: Lang; area?: number | null; perimeter?: number | null; length?: number | null; developer?: boolean; datasetTitle?: string }
type Props = Record<string, unknown>;

const value = (v: unknown): string => typeof v === "boolean" ? t(v ? "value.yes" : "value.no") : String(v);
const present = (v: unknown): v is string | number | boolean => v !== null && v !== undefined && v !== "";
const humanise = (v: string): string => v.replace(/[_-]+/g, " ").replace(/^./, (c) => c.toUpperCase());
/** Translate a coded value; a value without a translation is shown humanised (never as an i18n key). */
const translated = (prefix: string, v: unknown): string => {
  if (!present(v)) return "";
  const key = `${prefix}.${String(v)}`;
  return has(key) ? t(key as never) : humanise(String(v));
};

export function inspectorRows(group: string, properties: Props, ctx: InspectorContext): InspectorRow[] {
  const rows: InspectorRow[] = [];
  const add = (label: string, property: string, format: (v: unknown) => string = value) => {
    const v = properties[property];
    if (present(v)) rows.push({ label, value: format(v), raw: property });
  };
  const measured = (label: string, v: number | null | undefined, unit: string) => {
    if (v != null) rows.push({ label, value: `${fmtInt(Math.round(v * 10) / 10)} ${unit}` });
  };
  if (group === "buildings") {
    add(t("inspect.building"), "name"); measured(t("inspect.area"), ctx.area, "m²"); measured(t("inspect.perimeter"), ctx.perimeter, "m");
    add(t("inspect.height"), "height", (v) => `${v} m`); add(t("inspect.floors"), "num_floors"); add(t("inspect.type"), "subtype"); add(t("inspect.class"), "class");
  } else if (group === "transport") {
    add(t("inspect.road_name"), "name"); add(t("inspect.class"), "class", (v) => translated("transport.class", v)); add(t("inspect.type"), "subtype");
    add(t("inspect.surface"), "surface"); add(t("inspect.max_speed"), "max_speed", (v) => `${v}${properties.max_speed_unit ? ` ${properties.max_speed_unit}` : ""}`); measured(t("inspect.length"), ctx.length, "m");
  } else if (group === "admin") {
    add(t("inspect.unit_name"), "name"); add(t("inspect.admin_level"), "subtype", (v) => translated("admin.subtype", v)); add(t("inspect.code"), "division_id");
    if (!present(properties.division_id)) add(t("inspect.code"), "region"); measured(t("inspect.area"), ctx.area, "m²");
  } else if (group === "places") {
    add(t("inspect.name"), "name"); add(t("inspect.type"), "taxonomy"); add(t("inspect.category"), "category", (v) => translated("place.category", v));
    add(t("inspect.address"), "address"); add(t("inspect.locality"), "locality"); add(t("inspect.status"), "operating_status"); add(t("inspect.confidence"), "confidence", (v) => `${Math.round(Number(v) * 100)}%`);
  } else if (group === "thematic") {
    add(t("inspect.name"), "name"); add(t("inspect.landuse"), "class", (v) => translated("landuse.class", v)); add(t("inspect.class"), "subtype");
    add(t("inspect.surface"), "surface"); measured(t("inspect.area"), ctx.area, "m²");
  }
  const source = [ctx.datasetTitle, properties.source_dataset].filter(present).join(" · ");
  if (source) rows.push({ label: t("inspect.source"), value: source, raw: "source_dataset" });
  if (ctx.developer) for (const [raw, rawValue] of Object.entries(properties)) if (raw !== "__i" && raw !== "__layer" && !rows.some((r) => r.raw === raw)) rows.push({ label: raw, value: value(rawValue), raw });
  return rows;
}
