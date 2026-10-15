// Pure part of the runtime probe: wire types, memory classes, freshness. No browser or DuckDB imports,
// so it is unit-tested with node.

export interface ProbeCheck { name: string; ok: boolean; required: boolean; detail: string; ms: number }
export type MemoryClass = "low" | "standard" | "high" | "unknown";
/** What the planner needs about the device, nothing more (major version only; no user-agent string, no screen, no timezone). */
export interface DeviceProfile {
  browser_family: "chrome" | "edge" | "firefox" | "safari" | "other";
  browser_major: number | null;
  platform: "windows" | "macos" | "linux" | "android" | "ios" | "other";
  hardware_concurrency: number | null;
  wasm_simd: boolean;
  wasm_threads: boolean;
}

export interface NavigatorLike {
  userAgent?: string;
  platform?: string;
  hardwareConcurrency?: number;
  userAgentData?: { brands?: { brand: string; version: string }[]; platform?: string };
}

/** Build the profile from navigator-like data (pure: unit-tested without a browser). */
export function deviceProfileFrom(nav: NavigatorLike, wasmSimd: boolean, wasmThreads: boolean): DeviceProfile {
  const brands = nav.userAgentData?.brands ?? [];
  const ua = nav.userAgent ?? "";
  const brand = (re: RegExp) => brands.find((b) => re.test(b.brand));
  let family: DeviceProfile["browser_family"] = "other";
  let major: number | null = null;
  const edge = brand(/edge/i), chrome = brand(/google chrome/i);
  const num = (v: string | undefined) => { const n = parseInt(v ?? "", 10); return Number.isFinite(n) ? n : null; };
  if (edge) { family = "edge"; major = num(edge.version); }
  else if (chrome) { family = "chrome"; major = num(chrome.version); }
  else if (/Edg\/(\d+)/.test(ua)) { family = "edge"; major = num(/Edg\/(\d+)/.exec(ua)?.[1]); }
  else if (/Firefox\/(\d+)/.test(ua)) { family = "firefox"; major = num(/Firefox\/(\d+)/.exec(ua)?.[1]); }
  else if (/Chrome\/(\d+)/.test(ua) && !brands.some((b) => /brave|opera|vivaldi/i.test(b.brand))) { family = "chrome"; major = num(/Chrome\/(\d+)/.exec(ua)?.[1]); }
  else if (/Version\/(\d+).*Safari\//.test(ua)) { family = "safari"; major = num(/Version\/(\d+)/.exec(ua)?.[1]); }
  const p = (nav.userAgentData?.platform ?? nav.platform ?? "").toLowerCase();
  const platform: DeviceProfile["platform"] = /win/.test(p) ? "windows" : /android/.test(p) || /android/i.test(ua) ? "android" : /iphone|ipad|ios/.test(p) ? "ios" : /mac/.test(p) ? "macos" : /linux|x11/.test(p) ? "linux" : "other";
  return { browser_family: family, browser_major: major, platform, hardware_concurrency: nav.hardwareConcurrency ?? null, wasm_simd: wasmSimd, wasm_threads: wasmThreads };
}

/** Wire format = the Rust `geo_planner::ProbeReport` (serde, snake_case). */
export interface ProbeReport {
  engine: string; session: string; engine_version: string; runtime_version: string;
  captured_at_ms: number; duration_ms: number; checks: ProbeCheck[];
  memory_class: MemoryClass; device_memory_gb: number | null; js_heap_limit_mb: number | null;
  supported_ops: string[];
  device?: DeviceProfile;
}

/** Same thresholds as `geo_planner::memory_class_for`. */
export function memoryClassFor(gb: number | null | undefined): MemoryClass {
  if (gb == null) return "unknown";
  if (gb <= 2) return "low";
  if (gb < 8) return "standard";
  return "high";
}

export const PROBE_TTL_MS = 25 * 60_000; // below the planner's 30 min TTL: the client refreshes before it goes stale

export function isFresh(r: ProbeReport | null, now = Date.now(), ttlMs = PROBE_TTL_MS): boolean {
  return r != null && now - r.captured_at_ms <= ttlMs;
}

/** A probe is usable when every *required* check passed (optional checks are information). */
export function usable(r: ProbeReport): boolean {
  return r.checks.length > 0 && r.checks.every((c) => !c.required || c.ok);
}

export function sessionId(): string {
  return `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;
}

