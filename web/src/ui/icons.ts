// Small stroke icons (16x16) for the pipeline and for status marks. Built with the DOM API (no innerHTML).
// Meaning is never carried by colour alone: every status icon is paired with text by the caller.

const NS = "http://www.w3.org/2000/svg";

const PATHS: Record<string, string[]> = {
  source: ["M3 4c0-1.1 2.2-2 5-2s5 .9 5 2-2.2 2-5 2-5-.9-5-2z", "M3 4v8c0 1.1 2.2 2 5 2s5-.9 5-2V4", "M3 8c0 1.1 2.2 2 5 2s5-.9 5-2"],
  bbox_filter: ["M2 3h12l-4.5 5.5V13l-3-1.5V8.5z"],
  intersects: ["M2 3h12l-4.5 5.5V13l-3-1.5V8.5z"],
  filter: ["M2 3h12l-4.5 5.5V13l-3-1.5V8.5z"],
  project: ["M3 3v10", "M8 3v10", "M13 3v10"],
  aggregate: ["M12 3H4l4 5-4 5h8"],
  sort: ["M5 3v10", "M3 11l2 2 2-2", "M11 13V3", "M9 5l2-2 2 2"],
  limit: ["M3 5h10", "M3 8h10", "M3 11h5", "M11 10l2 2 2-2"],
  buffer: ["M8 8m-2 0a2 2 0 1 0 4 0a2 2 0 1 0-4 0", "M8 8m-5 0a5 5 0 1 0 10 0a5 5 0 1 0-10 0"],
  spatial_join: ["M6 8m-3.5 0a3.5 3.5 0 1 0 7 0a3.5 3.5 0 1 0-7 0", "M10 8m-3.5 0a3.5 3.5 0 1 0 7 0a3.5 3.5 0 1 0-7 0"],
  server: ["M2 3h12v4H2z", "M2 9h12v4H2z", "M4.5 5h.01", "M4.5 11h.01"],
  browser: ["M2 3h12v10H2z", "M2 6h12", "M4 4.5h.01", "M6 4.5h.01"],
  ok: ["M3 8.5l3.2 3.2L13 4.8"],
  warn: ["M8 2l6.5 11.5h-13z", "M8 6.5v3.2", "M8 11.6h.01"],
  fail: ["M4 4l8 8", "M12 4l-8 8"],
  info: ["M8 8m-6 0a6 6 0 1 0 12 0a6 6 0 1 0-12 0", "M8 7.2v4", "M8 4.9h.01"],
  chevron: ["M6 3l5 5-5 5"],
  play: ["M4.5 3l8 5-8 5z"],
  gear: ["M8 8m-2 0a2 2 0 1 0 4 0a2 2 0 1 0-4 0", "M8 1.8v2", "M8 12.2v2", "M1.8 8h2", "M12.2 8h2", "M3.6 3.6l1.4 1.4", "M11 11l1.4 1.4", "M12.4 3.6L11 5", "M5 11l-1.4 1.4"],
  help: ["M8 8m-6 0a6 6 0 1 0 12 0a6 6 0 1 0-12 0", "M6.2 6.3a1.8 1.8 0 1 1 2.6 1.6c-.5.3-.8.6-.8 1.2", "M8 11.6h.01"],
  clock: ["M8 8m-6 0a6 6 0 1 0 12 0a6 6 0 1 0-12 0", "M8 4.6V8l2.2 1.4"],
  table: ["M2 3h12v10H2z", "M2 6.5h12", "M2 10h12", "M6.5 3v10"],
  doc: ["M4 2h5l3 3v9H4z", "M9 2v3h3", "M6 8h4", "M6 10.5h4"],
  search: ["M7 7m-4.5 0a4.5 4.5 0 1 0 9 0a4.5 4.5 0 1 0-9 0", "M10.5 10.5L14 14"],
  grid: ["M2 2h5v5H2z", "M9 2h5v5H9z", "M2 9h5v5H2z", "M9 9h5v5H9z"],
  memory: ["M3 4h10v8H3z", "M5.5 4V2.5", "M8 4V2.5", "M10.5 4V2.5", "M5.5 12v1.5", "M8 12v1.5", "M10.5 12v1.5"],
  query: ["M3 4h10", "M3 8h6", "M3 12h10"],
  log: ["M3 3h10v10H3z", "M5.5 6l2 2-2 2", "M9 10.5h2"],
};

export function icon(name: string, size = 16): SVGSVGElement {
  const svg = document.createElementNS(NS, "svg");
  svg.setAttribute("viewBox", "0 0 16 16");
  svg.setAttribute("width", String(size));
  svg.setAttribute("height", String(size));
  svg.setAttribute("fill", "none");
  svg.setAttribute("stroke", "currentColor");
  svg.setAttribute("stroke-width", "1.5");
  svg.setAttribute("stroke-linecap", "round");
  svg.setAttribute("stroke-linejoin", "round");
  svg.setAttribute("aria-hidden", "true");
  svg.setAttribute("focusable", "false");
  for (const d of PATHS[name] ?? PATHS.info) {
    const p = document.createElementNS(NS, "path");
    p.setAttribute("d", d);
    svg.append(p);
  }
  return svg;
}
