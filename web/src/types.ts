// Wire types of the gateway responses (what the planner and the engines report). Types only: importable from tests.

import type { FailureClass } from "./engine/failure.ts";

export interface Column { name: string; type: string }
export interface SourceMeta {
  id: string; format: string; size_bytes: number; rows: number; row_groups: number; crs: string; crs_known: boolean;
  geometry_column: string; geometry_type: string; bbox: [number, number, number, number] | null; has_covering: boolean;
  covering_inferred?: boolean; modified?: string | null;
  columns: Column[]; error?: string;
}
export interface Decision { rule: string; subject: string; outcome: string; reason: string }
export interface Factor { name: string; value: number; weight: number; detail: string }
export interface Confidence { score: number; label: "high" | "medium" | "low"; factors: Factor[] }
export type Safety = "safe" | "risky" | "unsafe" | "unknown";
export interface Envelope { safe_fetch_bytes: number; unsafe_fetch_bytes: number; safe_result_bytes: number; unsafe_result_bytes: number; basis: string }
export interface Risk {
  safety: Safety; score: number; memory: number; result_rows: number; probe: number; health: number; format: number;
  memory_input_bytes: number; fetch_upper_bytes: number; result_upper_bytes: number; envelope: Envelope; reasons: string[];
}
export interface Health { state: "healthy" | "degraded" | "unhealthy" | "unknown"; reason: string; events: number; failures: number; last_failure: string | null }
export interface Candidate {
  engine: string; kind: "server" | "browser"; feasible: boolean; score_ms: number | null;
  cost: { fixed_ms: number; scan_ms: number; result_ms: number; transfer_ms: number; latency_ms: number; cold_start_ms: number; scan_bytes: number; transfer_bytes: number } | null;
  reasons: string[]; rejected_because: string[];
  confidence: Confidence | null; risk: Risk | null; health: Health; probe_status: string;
}
export interface Estimate {
  rows: number; low: number; high: number; quality: "poor" | "fair" | "good";
  cardinality: number; cardinality_low: number; cardinality_high: number; cardinality_quality: "poor" | "fair" | "good";
  bounded_by_limit: boolean; limit_certain: boolean;
  covering_quality: string; layout: string; raw_selected: number;
  correction: { factor: number; samples: number; basis: string; spread: number };
  spatial_coherence: number | null; row_groups_total: number; row_groups_kept: number; notes: string[];
}
export interface BrowserAccessInfo { uri: string; strategy: "full" | "range"; file_bytes: number; requests_est: number; fetch_bytes_est: number; fetch_bytes_upper: number; row_bytes: number }
export interface Assessment {
  confidence: Confidence; engine_confidence: number; margin_factor: number; estimate: Estimate;
  network: { name: string; rtt_ms: number; down_mbps: number; up_mbps: number; origin: string };
  health: Record<string, Health>;
  probes: Record<string, { status: string; usable: boolean; age_ms: number | null; failed_checks: string[]; memory_class: string | null; engine_version: string | null; runtime_version: string | null }>;
  fallback: { engine: string; reason: string; max_fallbacks: number } | null; policy: string[];
  browser_access: BrowserAccessInfo[];
}
export interface Scan {
  uri: string; total_bytes?: number; has_histogram?: boolean;
  covering?: { xmin: string; ymin: string; xmax: string; ymax: string; declared: boolean } | null;
  prune: { kept: number[]; skipped: number; total_row_groups: number; total_rows: number; kept_rows: number; kept_bytes: number; total_bytes: number };
}
export interface Physical {
  engine: string; placement?: "server" | "browser"; decisions: Decision[]; steps: { label: string; detail: string }[];
  assessment: Assessment;
  candidates?: Candidate[]; input_bytes_upper_bound?: number; result_rows_estimate?: number;
  scans: Scan[];
}
export interface Metric<T> { value: T | null; source: string }
export interface Metrics {
  elapsed_ms: Metric<number>; rows_returned: Metric<number>; row_groups_total: Metric<number>;
  row_groups_skipped: Metric<number>; rows_scanned_upper_bound: Metric<number>;
  estimated_bytes_upper_bound: Metric<number>; bytes_read: Metric<number>; peak_memory_bytes: Metric<number>;
  notes: string[];
}
export interface RunResp {
  fallback?: { from: string; reason: string; class: FailureClass };
  placement?: "server" | "browser"; result_digest?: { count: number; sha256: string };
  source_urls?: Record<string, string>; limits?: { max_rows: number; timeout_ms: number };
  plan_id: string; ir: unknown; physical: Physical; lineage: { id: string; op: string }[];
  columns: { name: string; type: string; geometry: boolean }[]; rows: Record<string, unknown>[]; rows_total: number;
  geojson: GeoJSON.FeatureCollection | null; metrics: Metrics;
  timing: { plan_and_overhead_ms: Metric<number>; total_ms: Metric<number>; boundary_conversion_ms: Metric<number> };
  /** Browser runs only: measured wall-clock phases (calibration evidence). */
  phases?: { init_ms: number; open_ms: number; first_ms: number; drain_ms: number; rows: number; boundary_ms: number };
}
