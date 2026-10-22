// Deployment profile, decided at build time: `vite build --mode static` defines __GF_PROFILE__ (see vite.config.ts).
// "static" = browser-only: no GeoFabric Gateway, no DuckDB server, no PostGIS; the page answers /api/* itself (static/gateway.ts).
// Anything else = the full stack (the Gateway serves /api/*), unchanged.
declare const __GF_PROFILE__: string | undefined;
export const STATIC_PROFILE: boolean = typeof __GF_PROFILE__ !== "undefined" && __GF_PROFILE__ === "static";
/**
 * Where the public demo opens: a neighbourhood-sized view of central Ho Chi Minh City (the pack's centre), not "fit the whole dataset". Fitting
 * the extent makes the first query cover every row group (the whole pack crosses the network even on a range-capable host) and shows
 * sub-pixel features; the demo is about the city, and a local first view reads only the row groups it needs.
 */
export const STATIC_START_VIEW = { center: [106.6985, 10.7765] as [number, number], zoom: 15.4 };
/** The app's base path (`/geofabric/` on GitHub Pages, `/` elsewhere). */
export const BASE_URL: string = (import.meta as unknown as { env?: { BASE_URL?: string } }).env?.BASE_URL ?? "/";
