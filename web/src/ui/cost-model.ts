// Developer mode: how an engine's cost model judged a plan (ADR-015). A pure function, so the wording and the ok/warn rule are unit tested;
// Explorer never calls it (normal users are not shown engine models, not even their absence).
import type { CostModelVerdict } from "../types.ts";

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
}

export function summarizeCostModel(m: CostModelVerdict): CostModelSummary {
  const top = m.terms
    .filter((x) => x.ms >= 0.5)
    .sort((a, b) => b.ms - a.ms)
    .slice(0, 3)
    .map((x) => `${x.name} ${x.ms.toFixed(0)} ms`)
    .join(", ");
  return {
    level: m.validation === "validated" && !m.risk.blocked && m.risk.level === "low" ? "ok" : "warn",
    predicted_ms: m.time_ms,
    workload_class: m.workload_class,
    validation: m.validation,
    risk: m.risk.blocked ? `${m.risk.level} (refused)` : m.risk.level,
    index_state: m.index_state,
    top_terms: top,
    model_version: m.model_version,
    risk_reason: m.risk.reason,
  };
}
