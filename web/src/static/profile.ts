// Deployment profile, decided at build time: `vite build --mode static` defines __GF_PROFILE__ (see vite.config.ts).
// "static" = browser-only: no GeoFabric Gateway, no DuckDB server, no PostGIS; the page answers /api/* itself (static/gateway.ts).
// Anything else = the full stack (the Gateway serves /api/*), unchanged.
declare const __GF_PROFILE__: string | undefined;
export const STATIC_PROFILE: boolean = typeof __GF_PROFILE__ !== "undefined" && __GF_PROFILE__ === "static";
/** The app's base path (`/geofabric/` on GitHub Pages, `/` elsewhere). */
export const BASE_URL: string = (import.meta as unknown as { env?: { BASE_URL?: string } }).env?.BASE_URL ?? "/";
