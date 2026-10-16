export const EARTH_RADIUS = 6371008.8;
const rad = (d: number) => d * Math.PI / 180;

function segment(a: number[], b: number[]): number {
  const dl = rad(b[0] - a[0]), dp = rad(b[1] - a[1]);
  const h = Math.sin(dp / 2) ** 2 + Math.cos(rad(a[1])) * Math.cos(rad(b[1])) * Math.sin(dl / 2) ** 2;
  return 2 * EARTH_RADIUS * Math.asin(Math.min(1, Math.sqrt(h)));
}
function lineLength(line: number[][]): number {
  return line.slice(1).reduce((n, point, i) => n + segment(line[i], point), 0);
}
export function lengthOf(geometry: GeoJSON.Geometry): number | null {
  if (geometry.type === "LineString") return lineLength(geometry.coordinates);
  if (geometry.type === "MultiLineString") return geometry.coordinates.reduce((n, line) => n + lineLength(line), 0);
  return null;
}
