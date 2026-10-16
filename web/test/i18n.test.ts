import assert from "node:assert/strict";
import { test } from "node:test";
import { dictionary, LANGS, setLang, t } from "../src/ui/i18n.ts";
import { datasetLabel, fieldLabel, fmtBytes, fmtMs, fmtPct, geometryLabel } from "../src/ui/labels.ts";

test("vi and en have exactly the same keys, none empty, and the same placeholders", () => {
  const vi = dictionary("vi"), en = dictionary("en");
  assert.deepEqual(Object.keys(vi).sort(), Object.keys(en).sort());
  const ph = (s: string) => [...s.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort().join(",");
  for (const k of Object.keys(vi)) {
    assert.ok(vi[k].trim() && en[k].trim(), `empty string for ${k}`);
    assert.equal(ph(vi[k]), ph(en[k]), `placeholders differ for ${k}`);
  }
  assert.deepEqual(LANGS, ["vi", "en"]);
});

test("t() substitutes placeholders and never throws on a missing parameter", () => {
  setLang("en");
  assert.equal(t("status.results", { n: 20000, time: "92 ms" }), "20000 results · 92 ms");
  assert.equal(t("status.results", { n: 1 }), "1 results · {time}");
  setLang("vi");
  assert.equal(t("status.results", { n: 20, time: "1 s" }), "20 kết quả · 1 s");
});

test("Explorer text never contains engine or planner jargon", () => {
  const jargon = /duckdb|wasm|postgis|planplan|plangraph|row group|credit/i;
  for (const lang of LANGS) {
    const d = dictionary(lang);
    for (const [k, v] of Object.entries(d)) {
      // developer-only strings are allowed to name things
      if (k.startsWith("dev.") || k.startsWith("metric.") || k === "data.row_groups" || k.startsWith("map.legend.") && k.endsWith("_dev") || k === "settings.mode_note" || k === "data.covering" || k.startsWith("data.covering_") || k === "query.ir" || k === "query.ir_note" || k === "top.engine" || k === "top.network" || k === "top.probe" || k.startsWith("profile.") || k === "settings.network" || k === "settings.network_note" || k === "settings.overlays") continue;
      assert.ok(!jargon.test(v), `${lang} ${k}: "${v}"`);
    }
  }
});

test("dataset labels are friendly and keep the shape of the data", () => {
  setLang("vi");
  const a = datasetLabel("clustered_100mb_spatial");
  assert.equal(a.title, "Tòa nhà — Lớn");
  assert.deepEqual(a.traits, ["tập trung theo cụm", "đã sắp xếp theo vị trí"]);
  const b = datasetLabel("uniform_10mb_shuffled_s2");
  assert.equal(b.title, "Tòa nhà — Nhỏ");
  assert.ok(b.traits.includes("xáo trộn") && b.traits.includes("bản sinh thứ hai"));
  assert.equal(datasetLabel("overture_buildings_vn_south").title, "Tòa nhà Overture");
  setLang("en");
  assert.equal(datasetLabel("clustered_1gb_spatial").title, "Buildings — Extra large");
  assert.equal(datasetLabel("my_roads-2026").title, "My roads 2026");
  setLang("vi");
});

test("field and geometry labels: known ones translated, unknown ones humanised, never hidden", () => {
  setLang("vi");
  assert.equal(fieldLabel("id"), "Mã đối tượng");
  assert.equal(fieldLabel("class"), "Phân loại");
  assert.equal(fieldLabel("roof_shape"), "Hình dạng mái");
  assert.equal(fieldLabel("some_new_field"), "Some new field");
  assert.equal(geometryLabel("Polygon"), "Vùng");
  assert.equal(geometryLabel("GeometryCollection"), "GeometryCollection");
  setLang("en");
  assert.equal(fieldLabel("id"), "Feature ID");
  setLang("vi");
});

test("number formats follow the language", () => {
  setLang("vi");
  assert.equal(fmtBytes(26_000_000).replace(/\s/g, " "), "24,8 MB");
  assert.equal(fmtMs(92), "92,0 ms");
  assert.equal(fmtMs(2500), "2,50 s");
  assert.equal(fmtPct(0.983), "98,3%");
  setLang("en");
  assert.equal(fmtBytes(26_000_000), "24.8 MB");
  assert.equal(fmtMs(92), "92.0 ms");
  assert.equal(fmtPct(0.983), "98.3%");
  setLang("vi");
});
