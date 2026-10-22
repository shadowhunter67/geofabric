// Browser loader of the lowering wasm (Vite resolves the asset URL, so the module works under any base path such as /geofabric/).
import loweringWasm from "./generated/geo_lowering.wasm?url";
import { initLowering, loweringReady } from "./lowering.ts";

let loading: Promise<void> | null = null;

/** Load the lowering module once; every later call returns the same promise (a failed load is retried on the next call). */
export function ensureLowering(): Promise<void> {
  if (loweringReady()) return Promise.resolve();
  loading ??= initLowering(fetch(loweringWasm)).catch((e) => {
    loading = null;
    throw e;
  });
  return loading;
}
