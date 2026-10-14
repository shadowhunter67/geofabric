// Pure part of the runtime probe: wire types, memory classes, freshness. No browser or DuckDB imports,
// so it is unit-tested with node.

export interface ProbeCheck { name: string; ok: boolean; required: boolean; detail: string; ms: number }
export type MemoryClass = "low" | "standard" | "high" | "unknown";
/** Wire format = the Rust `geo_planner::ProbeReport` (serde, snake_case). */
export interface ProbeReport {
  engine: string; session: string; engine_version: string; runtime_version: string;
  captured_at_ms: number; duration_ms: number; checks: ProbeCheck[];
  memory_class: MemoryClass; device_memory_gb: number | null; js_heap_limit_mb: number | null;
  supported_ops: string[];
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

