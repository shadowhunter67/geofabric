// The environment of the gateway that the full-stack suite starts.
//
// The engine-state tests (Developer mode says PostGIS is "not configured", the planner shows exactly one alternative, ...) describe a gateway WITHOUT
// the optional PostGIS engine. A shell that still exports GEOFABRIC_POSTGIS_* from an earlier live session (2026-10-23: three failures on both browsers
// until the variables were unset) must not change what the suite measures, so the suite blanks every such variable explicitly instead of inheriting it.
// A run that WANTS PostGIS opts in with GF_E2E_POSTGIS=1 and then inherits the variables unchanged.

/** Every variable the gateway reads for PostGIS (crates/geo-engine-postgis/src/config.rs) - and anything else that starts with the prefix. */
export const KNOWN_POSTGIS_ENV = ["GEOFABRIC_POSTGIS_ENABLED", "GEOFABRIC_POSTGIS_URL", "GEOFABRIC_POSTGIS_SOURCES", "GEOFABRIC_POSTGIS_POOL"] as const;

export function gatewayEnv(base: Record<string, string | undefined> = process.env): Record<string, string> {
  const env: Record<string, string> = { CARGO_BUILD_JOBS: "3" };
  if (base.GF_E2E_POSTGIS === "1") return env; // explicit opt-in: inherit
  const keys = new Set<string>([...KNOWN_POSTGIS_ENV, ...Object.keys(base).filter((k) => k.startsWith("GEOFABRIC_POSTGIS_"))]);
  for (const k of keys) env[k] = ""; // an empty value is "not enabled" (ENABLED must be exactly 1/true), and the URL/sources are never read
  return env;
}
