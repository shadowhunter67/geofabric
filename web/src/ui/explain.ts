// Explorer-level explanation of a run: what was done, where, and why, in terms a non-specialist understands.
// Pure functions over the gateway response (unit-tested); no DOM, no engine names.

import type { PlanGraph } from "../engine/contract.ts";
import type { Key } from "./i18n.ts";
import type { Candidate, RunResp } from "../types.ts";

export interface PipelineStep { key: string; title: Key; sub: Key }

const KNOWN = new Set(["source", "bbox_filter", "intersects", "filter", "project", "aggregate", "sort", "limit", "buffer", "spatial_join", "transform", "spatial_semi_join"]);

/** Steps of the plan from the data source to the root, then where it ran. Follows `input` (and `left` of a join). */
export function pipelineSteps(ir: PlanGraph | null | undefined, where: "server" | "browser"): PipelineStep[] {
  const steps: PipelineStep[] = [];
  if (ir) {
    let id: string | undefined = ir.root;
    const seen = new Set<string>();
    while (id && !seen.has(id)) {
      seen.add(id);
      const n = ir.nodes[id] as { op: string; input?: string; left?: string } | undefined;
      if (!n) break;
      const op = KNOWN.has(n.op) ? n.op : "filter";
      steps.push({ key: n.op, title: `pipe.${op}` as Key, sub: (`pipe.sub.${op}` as Key) });
      id = n.input ?? n.left;
    }
    steps.reverse();
  }
  steps.push({ key: where, title: `pipe.${where}` as Key, sub: `pipe.sub.${where}` as Key });
  return steps;
}

export interface ExecSummary {
  where: "server" | "browser";
  reasons: Key[];
  alt: { where: "server" | "browser"; timeMs: number | null; reason: Key | null } | null;
  timeMs: number;
  rows: number;
  /** Bytes of data the analysis had to process (estimate, from file statistics). */
  processBytes: number | null;
  /** Fraction of the file that did not have to be read (estimate), or null when nothing could be skipped. */
  skippedFraction: number | null;
  /** Browser memory the analysis needs (estimate); null when it ran on the server. */
  memoryBytes: number | null;
  fallback: boolean;
}

function reasonKey(c: Candidate): Key {
  const t = c.rejected_because.join(" ").toLowerCase();
  if (/unsafe|memory|too large|exceeds the engine limit/.test(t)) return "exec.reason.too_large";
  if (/unsupported operation|not supported|covering/.test(t)) return "exec.reason.unsupported";
  if (/disabled/.test(t)) return "exec.reason.disabled";
  if (/probe/.test(t)) return "exec.reason.unverified";
  if (/unhealthy|failed before/.test(t)) return "exec.reason.unhealthy";
  return "exec.reason.generic";
}

/** `chosen` = the user picked the engine explicitly (an engine hint), so "why" is simply "you chose it". */
export function summarize(r: RunResp, chosen = false): ExecSummary {
  const p = r.physical;
  const where: "server" | "browser" = p.placement === "browser" || p.engine === "duckdb-wasm" ? "browser" : "server";
  const sel = p.candidates?.find((c) => c.engine === p.engine);
  const other = p.candidates?.find((c) => c.engine !== p.engine);
  const reasons: Key[] = [];
  let alt: ExecSummary["alt"] = null;
  if (other) {
    alt = { where: other.kind === "browser" ? "browser" : "server", timeMs: other.score_ms, reason: other.feasible ? null : reasonKey(other) };
  }
  const fallback = !!r.fallback;
  if (fallback) reasons.push("exec.why.fallback");
  else if (chosen) reasons.push("exec.why.chosen");
  else if (!other || !other.feasible) reasons.push("exec.why.only");
  else {
    const cheaper = sel?.score_ms != null && other.score_ms != null && sel.score_ms <= other.score_ms;
    if (p.assessment?.policy?.length && !cheaper) reasons.push("exec.why.cautious");
    else if (cheaper) reasons.push("exec.why.faster");
    if (where === "server") {
      const risk = other.risk;
      if (risk && risk.safety !== "safe") reasons.push("exec.why.safer");
    } else {
      reasons.push("exec.why.no_server");
    }
  }
  const totalBytes = p.scans.reduce((a, s) => a + (s.total_bytes ?? s.prune.total_bytes), 0);
  const keptBytes = p.scans.reduce((a, s) => a + s.prune.kept_bytes, 0);
  const skipped = totalBytes > 0 && keptBytes < totalBytes ? 1 - keptBytes / totalBytes : null;
  const browserCand = p.candidates?.find((c) => c.kind === "browser");
  return {
    where,
    reasons,
    alt,
    timeMs: r.timing.total_ms.value ?? 0,
    rows: r.rows_total,
    processBytes: p.assessment?.browser_access?.length && where === "browser" ? p.assessment.browser_access.reduce((a, x) => a + x.fetch_bytes_est, 0) : keptBytes || null,
    skippedFraction: skipped,
    memoryBytes: where === "browser" ? (browserCand?.risk?.memory_input_bytes ?? null) : null,
    fallback,
  };
}
