import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

// WCAG contrast of the colour tokens in style.css, for both colour schemes: text on every surface >= 4.5:1,
// status colours / accent >= 4.5:1 on the panel, focus ring >= 3:1.

const css = readFileSync(new URL("../src/style.css", import.meta.url), "utf8");

function tokens(block: string): Record<string, string> {
  return Object.fromEntries([...block.matchAll(/--([a-z0-9-]+):\s*(#[0-9a-fA-F]{6})/g)].map((m) => [m[1], m[2]]));
}
const dark = tokens(css.slice(css.indexOf(":root {"), css.indexOf("@media")));
const light = { ...dark, ...tokens(css.slice(css.indexOf("@media (prefers-color-scheme: light)"), css.indexOf("* { box-sizing"))) };

function lum(hex: string): number {
  const c = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255).map((v) => (v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4));
  return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
}
function contrast(a: string, b: string): number {
  const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p);
  return (x + 0.05) / (y + 0.05);
}

test("contrast helper is right (black on white = 21:1)", () => {
  assert.ok(Math.abs(contrast("#000000", "#ffffff") - 21) < 1e-6);
});

for (const [name, t] of [["dark", dark], ["light", light]] as const) {
  test(`${name} theme: text and muted text are readable on every surface`, () => {
    for (const bg of ["bg", "panel", "panel-2"]) {
      assert.ok(contrast(t.text, t[bg]) >= 4.5, `text on ${bg}: ${contrast(t.text, t[bg]).toFixed(2)}`);
      assert.ok(contrast(t.muted, t[bg]) >= 4.5, `muted on ${bg}: ${contrast(t.muted, t[bg]).toFixed(2)}`);
    }
  });
  test(`${name} theme: status colours, accent and focus ring are distinguishable from the panels`, () => {
    for (const k of ["ok", "bad", "accent-2", "accent"]) {
      assert.ok(contrast(t[k], t["panel"]) >= 4.5, `${k} on panel: ${contrast(t[k], t["panel"]).toFixed(2)}`);
      assert.ok(contrast(t[k], t["panel-2"]) >= 4.5, `${k} on panel-2: ${contrast(t[k], t["panel-2"]).toFixed(2)}`);
    }
    assert.ok(contrast(t.focus, t["panel"]) >= 3, `focus ring: ${contrast(t.focus, t["panel"]).toFixed(2)}`);
    assert.ok(contrast(t["on-accent"], t.accent) >= 4.5, `text on the primary button: ${contrast(t["on-accent"], t.accent).toFixed(2)}`);
  });
}
