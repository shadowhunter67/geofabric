// Developer mode: how an engine's cost model judged a plan (ADR-015). Pure functions, so the wording and the ok/warn rules are unit tested;
// Explorer never calls them (normal users are not shown engine models, not even their absence).
import type { CostModelVerdict, SemiJoinInfo } from "../types.ts";

export interface CostModelSummary {
  /** `ok` only for a Validated class with a low, unblocked risk; everything else is a warning the reader should see */
  level: "ok" | "warn";
  predicted_ms: number;
  workload_class: string;
  validation: CostModelVerdict["validation"];
  risk: string;
  index_state: string;
  /** the three largest cost terms, e.g. `startup 16 ms, scan 9 ms` */
  top_terms: string;
  model_version: string;
  /** the hard-policy reason when the workload is refused or elevated, else "" */
  risk_reason: string;
  /** "" when the engine states no timeout opinion; `unknown` is shown as such, never as 0 */
  timeout: string;
}

export function summarizeCostModel(m: CostModelVerdict): CostModelSummary {
  const top = m.terms
    .filter((x) => x.ms >= 0.5)
    .sort((a, b) => b.ms - a.ms)
    .slice(0, 3)
    .map((x) => `${x.name} ${x.ms.toFixed(0)} ms`)
    .join(", ");
  const p = m.risk.timeout_probability;
  const timeout = p != null ? `${(p * 100).toFixed(p < 0.1 ? 1 : 0)} %` : m.risk.level === "unknown" ? "unknown" : "";
  return {
    level: m.validation === "validated" && !m.risk.blocked && m.risk.level === "low" ? "ok" : "warn",
    predicted_ms: m.time_ms,
    workload_class: m.workload_class,
    validation: m.validation,
    risk: m.risk.blocked ? `${m.risk.level} (refused${m.risk.overridable ? ", liftable" : ""})` : m.risk.level,
    index_state: m.index_state,
    top_terms: top,
    model_version: m.model_version,
    risk_reason: m.risk.reason,
    timeout,
  };
}

const num = (v: number | null | undefined): string => (v == null ? "unknown" : v >= 100 ? v.toLocaleString("en-US", { maximumFractionDigits: 0 }) : v.toPrecision(3));

/** One line about the candidate-work features of a metric semi join, with unknown shown as unknown. `null` for a plan that is not a metric semi join. */
export function semiJoinLine(s: SemiJoinInfo | null | undefined): { text: string; unknownFeatures: string[] } | null {
  if (!s || !s.metric) return null;
  const unknown: string[] = [];
  const pick = (name: string, v: number | null | undefined): string => {
    if (v == null) unknown.push(name);
    return num(v);
  };
  const text = [
    `probes ${num(s.expected_probes)}`,
    `right (filtered) ${pick("right_rows_filtered", s.right_rows_filtered)}`,
    `distance ${s.distance_m == null ? "unknown" : `${s.distance_m} m`}`,
    `candidates/probe ${pick("candidates_per_probe", s.candidates_per_probe)}`,
    `candidate pairs ${pick("candidate_pairs", s.candidate_pairs)}`,
    `overlap ${pick("overlap_ratio", s.overlap_ratio)}`,
    `basis ${s.estimate_basis ?? "unknown"}`,
  ].join(" · ");
  return { text, unknownFeatures: unknown };
}

/** What the planner says when it could not establish that the chosen engine is safe (empty = nothing to say). */
export function uncertaintyLines(u: { state: string; policy_version: string; reasons: string[] } | null | undefined): string[] {
  return u && u.reasons.length ? u.reasons.map((r) => `${r} (${u.policy_version})`) : [];
}
