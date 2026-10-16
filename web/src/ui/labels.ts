// Human-facing labels for datasets, fields and numbers. Pure functions (unit-tested under node).
// Explorer mode shows these; Developer mode shows the raw names next to them.

import { dictionary, getLang, t } from "./i18n.ts";
import type { Key } from "./i18n.ts";

const has = (k: string): k is Key => k in dictionary(getLang());

export function locale(): string { return getLang() === "vi" ? "vi-VN" : "en-US"; }

export function fmtInt(n: number): string { return n.toLocaleString(locale()); }

export function fmtBytes(b: number | null | undefined): string {
  if (b == null) return "—";
  const dec = (v: number, d = 1) => v.toLocaleString(locale(), { minimumFractionDigits: d, maximumFractionDigits: d });
  if (b < 1024) return `${b} B`;
  if (b < 1048576) return `${dec(b / 1024)} KB`;
  if (b < 1073741824) return `${dec(b / 1048576)} MB`;
  return `${dec(b / 1073741824, 2)} GB`;
}

export function fmtMs(v: number): string {
  const dec = (x: number, d: number) => x.toLocaleString(locale(), { minimumFractionDigits: d, maximumFractionDigits: d });
  return v < 1000 ? `${dec(v, v < 100 ? 1 : 0)} ms` : `${dec(v / 1000, 2)} s`;
}

export function fmtPct(f: number): string {
  return `${(f * 100).toLocaleString(locale(), { minimumFractionDigits: 1, maximumFractionDigits: 1 })}%`;
}

export interface DatasetLabel {
  /** Short, friendly name. */
  title: string;
  /** Qualifiers (shape of the data), shown in a smaller line. */
  traits: string[];
  /** Where the data comes from (a translated sentence). */
  origin: string;
}

const SIZE: Record<string, Key> = { "10mb": "data.size.small", "30mb": "data.size.medium", "100mb": "data.size.large", "1gb": "data.size.xlarge" };

/** The datasets Explorer lists: the size ladder of the synthetic buildings (spatially sorted) and the real Overture subset. */
export function isPrimaryDataset(id: string): boolean {
  return /^clustered_(10mb|30mb|100mb|1gb)_spatial$/.test(id) || id.startsWith("overture_");
}

/** Friendly label of a dataset id such as `clustered_100mb_spatial` or `overture_buildings_vn_south`. */
export function datasetLabel(id: string): DatasetLabel {
  const syn = /^(clustered|uniform)_(10mb|30mb|100mb|1gb)_(spatial|shuffled)(_s2)?$/.exec(id);
  if (syn) {
    const [, dist, size, order, second] = syn;
    return {
      title: `${t("data.buildings")} — ${t(SIZE[size])}`,
      traits: [t(dist === "clustered" ? "data.dist.clustered" : "data.dist.uniform"), t(order === "spatial" ? "data.layout.sorted" : "data.layout.shuffled"), ...(second ? [t("data.second_draw")] : [])],
      origin: t("data.origin.synthetic"),
    };
  }
  if (id.startsWith("overture_buildings")) return { title: t("data.buildings_overture"), traits: [], origin: t("data.origin.overture") };
  return { title: id.replace(/[_-]+/g, " ").replace(/^./, (c) => c.toUpperCase()), traits: [], origin: t("data.origin.other") };
}

/** Friendly label of a column / property name; unknown names are humanised (never hidden). */
export function fieldLabel(name: string): string {
  const k = `field.${name}`;
  if (has(k)) return t(k as Key);
  return name.replace(/[_-]+/g, " ").replace(/^./, (c) => c.toUpperCase());
}

export function geometryLabel(type: string): string {
  const k = `geom.${type}`;
  return has(k) ? t(k as Key) : type;
}

/** Short relative-free date for the "updated" line (ISO date only: no time-zone surprises). */
export function fmtDate(iso: string | null | undefined): string {
  if (!iso) return "—";
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? "—" : d.toLocaleDateString(locale(), { year: "numeric", month: "2-digit", day: "2-digit" });
}
