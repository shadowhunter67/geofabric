import assert from "node:assert/strict";
import { test } from "node:test";
import { gatewayEnv, KNOWN_POSTGIS_ENV } from "../e2e/gateway-env.ts";

// Regression (2026-10-23): GEOFABRIC_POSTGIS_* left in the shell by a live PostGIS session made the full-stack suite's gateway see PostGIS as
// configured, which failed three engine-state tests on both browsers. The suite now starts its gateway with those variables blanked.

test("every PostGIS variable, known or merely prefixed, is blanked for the full-stack gateway", () => {
  const env = gatewayEnv({ GEOFABRIC_POSTGIS_ENABLED: "1", GEOFABRIC_POSTGIS_URL: "postgres://x", GEOFABRIC_POSTGIS_FUTURE_KNOB: "y", PATH: "/bin" });
  for (const k of KNOWN_POSTGIS_ENV) assert.equal(env[k], "", k);
  assert.equal(env.GEOFABRIC_POSTGIS_FUTURE_KNOB, "", "a variable this code has never heard of is blanked too");
  assert.equal(env.PATH, undefined, "unrelated variables are not touched (the child inherits them)");
  assert.equal(env.CARGO_BUILD_JOBS, "3");
});

test("an explicit opt-in (GF_E2E_POSTGIS=1) inherits the variables unchanged", () => {
  const env = gatewayEnv({ GF_E2E_POSTGIS: "1", GEOFABRIC_POSTGIS_ENABLED: "1" });
  assert.equal(env.GEOFABRIC_POSTGIS_ENABLED, undefined);
  assert.equal(env.CARGO_BUILD_JOBS, "3");
});

test("with nothing exported the gateway still gets the known variables blanked", () => {
  const env = gatewayEnv({});
  assert.deepEqual(KNOWN_POSTGIS_ENV.map((k) => env[k]), ["", "", "", ""]);
});
