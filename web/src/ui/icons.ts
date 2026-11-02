// GeoFabric icon set: 16x16 line icons, stroke 1.5, round caps and joins, drawn from GIS primitives (vertices, edges, rectangles, circles).
// A zero-length path ("M x y h.01") is a round VERTEX DOT, the set's signature. Built with the DOM API (no innerHTML).
// Meaning is never carried by colour alone: every status icon is paired with text by the caller.

const NS = "http://www.w3.org/2000/svg";

const PATHS: Record<string, string[]> = {
  // plan steps (pipeline)
  source: ["M2.5 4C2.5 2.6 5 1.75 8 1.75C11 1.75 13.5 2.6 13.5 4V12C13.5 13.4 11 14.25 8 14.25C5 14.25 2.5 13.4 2.5 12Z", "M2.5 4C2.5 5.4 5 6.25 8 6.25C11 6.25 13.5 5.4 13.5 4", "M2.5 8C2.5 9.4 5 10.25 8 10.25C11 10.25 13.5 9.4 13.5 8", "M8 4h.01"],
  bbox_filter: ["M3 3H13V13H3Z", "M1.75 5V1.75H5M11 1.75H14.25V5M14.25 11V14.25H11M5 14.25H1.75V11", "M8.5 7.5h.01"],
  intersects: ["M2 2H10V10H2Z", "M6 6H14V14H6Z", "M10 6h.01", "M6 10h.01"],
  filter: ["M1.75 2.25H14.25L9.5 8.25V13.75L6.5 12.25V8.25Z", "M8 2.25h.01"],
  project: ["M2.5 4V12", "M13.5 4V12", "M6 2.5H10V13.5H6Z", "M8 8h.01"],
  aggregate: ["M2 3.25L7.75 8M2 8H7.75M2 12.75L7.75 8L13.75 8", "M13.75 8L11.5 5.75M13.75 8L11.5 10.25", "M2 3.25h.01", "M2 8h.01", "M2 12.75h.01"],
  sort: ["M2.5 13V10M5.5 13V7M8.5 13V4", "M12.5 3V13M10.5 11L12.5 13L14 11", "M8.5 4h.01"],
  limit: ["M2 3H14M2 6H14M2 9H8", "M2 13H14", "M8 9h.01"],
  buffer: ["M8 2A6 6 0 1 1 8 14A6 6 0 1 1 8 2Z", "M8 5A3 3 0 1 1 8 11A3 3 0 1 1 8 5Z", "M8 8h.01"],
  spatial_join: ["M1.75 3H6.75V8H1.75Z", "M9.25 8H14.25V13H9.25Z", "M6.75 6.5L9.25 9.5", "M6.75 6.5h.01", "M9.25 9.5h.01"],
  server: ["M2 2H14V14H2Z", "M2 8H14M4 5H9M4 11H9", "M12 5h.01", "M12 11h.01"],
  browser: ["M2 2H14V14H2ZM2 5H14", "M4 3.5h.01", "M7 7L12 11L9.5 11.5L8.5 14Z"],
  // geometry and layers
  pin: ["M8 14C8 14 13 9.5 13 6.5C13 3.74 10.76 2 8 2C5.24 2 3 3.74 3 6.5C3 9.5 8 14 8 14Z", "M8 6.5h.01"],
  line: ["M2 12L6 5L10 10L14 3", "M2 12h.01", "M6 5h.01", "M14 3h.01"],
  polygon: ["M2 11L4 3L11 2L14 7L11 14L5 13Z", "M4 3h.01", "M14 7h.01", "M5 13h.01"],
  layers: ["M2 5L8 2L14 5L8 8Z", "M2 8L8 11L14 8", "M2 11L8 14L14 11", "M8 2h.01"],
  // status marks
  ok: ["M2.25 8.25 L6.25 12.25 L13.75 3.75"],
  warn: ["M8 2 L14 13 H2 Z", "M8 5.25 V9.25", "M8 11.25 h.01"],
  fail: ["M2.5 2.5 L13.5 13.5", "M13.5 2.5 L2.5 13.5"],
  info: ["M8 2 A6 6 0 1 1 8 14 A6 6 0 1 1 8 2 Z", "M8 7 V11.5", "M8 4.5 h.01"],
  chevron: ["M3.5 2.25 L12 8 L3.5 13.75"],
  // interface
  play: ["M3 2 L14 8 L3 14 Z"],
  gear: ["M8 1.75 L9 3.1 L10.65 2.65 L11 4.25 L12.65 4.6 L12.2 6.25 L14.25 7 L13.1 8 L14.25 9 L12.2 9.75 L12.65 11.4 L11 11.75 L10.65 13.35 L9 12.9 L8 14.25 L7 12.9 L5.35 13.35 L5 11.75 L3.35 11.4 L3.8 9.75 L1.75 9 L2.9 8 L1.75 7 L3.8 6.25 L3.35 4.6 L5 4.25 L5.35 2.65 L7 3.1 Z", "M8 5.75 A2.25 2.25 0 1 1 8 10.25 A2.25 2.25 0 1 1 8 5.75 Z"],
  help: ["M8 2 A6 6 0 1 1 8 14 A6 6 0 1 1 8 2 Z", "M5.75 6.25 C5.75 3.5 10.25 3.5 10.25 6.25 C10.25 8.25 8 8.25 8 10", "M8 12 h.01"],
  clock: ["M8 2 A6 6 0 1 1 8 14 A6 6 0 1 1 8 2 Z", "M8 4.25 V8 L11.25 10"],
  table: ["M2 2 H14 V14 H2 Z", "M2 5.5 H14", "M2 9.75 H14", "M6 5.5 V14", "M10 5.5 V14"],
  doc: ["M3 2 H10 L13 5 V14 H3 Z", "M10 2 V5 H13", "M5 7.5 H11", "M5 10 H11", "M5 12.5 H9"],
  search: ["M6.5 2.25 A4.25 4.25 0 1 1 6.5 10.75 A4.25 4.25 0 1 1 6.5 2.25 Z", "M9.5 9.5 L13.75 13.75"],
  grid: ["M2 2 H14 V14 H2 Z", "M8 2 V14", "M2 8 H14"],
  memory: ["M3 4 H13 V12 H3 Z", "M5.5 6 H10.5 V10 H5.5 Z", "M5 4 V2 M8 4 V2 M11 4 V2 M5 12 V14 M8 12 V14 M11 12 V14 M3 6 H2 M3 9 H2 M3 11 H2 M13 6 H14 M13 9 H14 M13 11 H14"],
  query: ["M2.5 4 H13.5", "M2.5 8 H10.5", "M2.5 12 H12.5", "M2.5 4 h.01 M2.5 8 h.01 M2.5 12 h.01"],
  log: ["M2 2 H14 V14 H2 Z", "M2 5 H14", "M4 8 L6.5 10 L4 12", "M8 12 H12"],
  // brand mark (woven diamond lattice)
  brand: ["M8 1.75L14.25 8L8 14.25L1.75 8Z", "M5 5L6.8 6.8M9.2 9.2L11 11", "M11 5L5 11", "M8 8h.01"],
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
