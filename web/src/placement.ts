// Where a plan runs and what happens when that fails: the glue between the gateway's planner, the browser engine, the runtime
// probe and the failure policy (ADR-009/010). No DOM here; the UI passes a logger and a "probe changed" callback.

import { DuckDbWasmEngine } from "./engine/wasm.ts";
import { resultDigest } from "./engine/digest.ts";
import { wkbToGeoJSON } from "./engine/wkb.ts";
import { EngineError } from "./engine/contract.ts";
import type { PlanGraph, SourceBinding } from "./engine/contract.ts";
import { isFresh, runProbe, sessionId, usable } from "./engine/probe.ts";
import type { ProbeReport } from "./engine/probe.ts";
import { classifyFailure, fallbackAllowed } from "./engine/failure.ts";
import { LayerUnavailable } from "./static/errors.ts";
import type { Metric, Metrics, RunResp, Scan } from "./types.ts";

export interface PlacementDeps {
  log(msg: string, isErr?: boolean): void;
  /** The probe state changed (started, finished): the UI may want to re-render. */
  onProbe(): void;
}

export function createPlacement(deps: PlacementDeps) {
  const wasm = new DuckDbWasmEngine();
  const SESSION = sessionId();
  const st = { probe: null as ProbeReport | null, probing: null as Promise<void> | null };

  /** Probe the browser engine and tell the planner. Re-runs on reload (new session), TTL expiry and after a fatal failure. */
  function probeRuntime(reason: string): Promise<void> {
    if (st.probing) return st.probing;
    st.probing = (async () => {
      try {
        const report = await runProbe(wasm, SESSION);
        st.probe = report;
        await fetch("/api/probe", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(report) });
        deps.log(`runtime probe (${reason}): ${usable(report) ? "usable" : "NOT usable"}, ${report.checks.filter((c) => c.ok).length}/${report.checks.length} checks, ${report.duration_ms.toFixed(0)} ms, memory class ${report.memory_class}, ${report.device?.browser_family ?? "?"} ${report.device?.browser_major ?? ""}`);
      } catch (e) {
        deps.log(`runtime probe failed: ${e}`, true);
      } finally {
        st.probing = null;
        deps.onProbe();
      }
    })();
    return st.probing;
  }

  /** Re-probe in the background when there is no fresh probe (TTL expiry, page just loaded). */
  function ensureFreshProbe(): void {
    if (!st.probing && !isFresh(st.probe)) void probeRuntime(st.probe ? "ttl expired" : "no probe yet");
  }

  function bindSources(r: RunResp): Record<string, SourceBinding> {
    const out: Record<string, SourceBinding> = {};
    for (const [uri, url] of Object.entries(r.source_urls ?? {})) {
      const scan = r.physical.scans.find((x) => x.uri === uri);
      out[uri] = { url, covering: scan?.covering ?? null, access: r.physical.assessment?.browser_access?.find((a) => a.uri === uri)?.strategy ?? "range" };
    }
    return out;
  }

  /** Fire-and-forget: tell the gateway how a browser run went (health, known incompatibilities, telemetry). */
  function reportRun(r: RunResp, ok: boolean, rowsOrErr: number | string, ms: number, fallback = false) {
    const scan = r.physical.scans[0];
    const source = scan ? scan.uri.split(/[\\/]/).pop()!.replace(/\.parquet$/, "") : "";
    const est = r.physical.assessment?.estimate;
    const nodes = Object.values((r.ir as PlanGraph).nodes);
    const agg = nodes.find((n) => n.op === "aggregate");
    const lim = nodes.filter((n) => n.op === "limit").map((n) => (n.op === "limit" ? n.n : Infinity));
    const groupBy = agg && agg.op === "aggregate" && agg.group_by.length ? agg.group_by[0] : undefined;
    const body = ok
      ? {
          engine: r.physical.engine, source, ok: true, kind: groupBy ? "count_by" : agg ? "count" : "features", raw_selected: est?.raw_selected ?? 0,
          total_rows: r.physical.scans.reduce((a, x) => a + (x.prune?.total_rows ?? 0), 0), est_rows: r.physical.result_rows_estimate ?? 0,
          actual_rows: rowsOrErr as number, limit: lim.length ? Math.min(...lim) : undefined, group_by: groupBy,
          est_ms: r.physical.candidates?.find((c) => c.engine === r.physical.engine)?.score_ms ?? undefined, actual_ms: ms, fallback, layout: est?.layout,
        }
      : { engine: r.physical.engine, source, ok: false, message: String(rowsOrErr) };
    void fetch("/api/report", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) }).catch(() => {});
  }

  /** If the planner placed the plan in the browser, run it there and return a server-shaped response. */
  async function resolvePlacement(r: RunResp): Promise<RunResp> {
    if (r.placement !== "browser") return r;
    const t0 = performance.now();
    const plan = r.ir as PlanGraph;
    let res;
    try {
      res = await wasm.execute(plan, { sources: bindSources(r), timeoutMs: r.limits?.timeout_ms ?? 30000, maxRows: r.limits?.max_rows ?? 100000, estimatedInputBytes: r.physical.input_bytes_upper_bound });
    } catch (e) {
      const k = e instanceof EngineError ? `${e.kind}: ` : "";
      throw new Error(`browser engine failed (${k}${e instanceof Error ? e.message : String(e)})`);
    }
    const t1 = performance.now();
    const srcNode = Object.values(plan.nodes).find((n) => n.op === "source");
    const geomName = srcNode && srcNode.op === "source" ? srcNode.schema.geometry_columns[0]?.name : undefined;
    const geomIdx = res.columns.findIndex((c) => c.name === geomName);
    const scalarCols = res.columns.map((c, i) => ({ c, i })).filter(({ i }) => {
      const v = res.rows.find((row) => row[i] !== null && row[i] !== undefined)?.[i];
      return i !== geomIdx && !(v instanceof Uint8Array) && !(typeof v === "object" && v !== null);
    });
    const cell = (v: unknown) => (typeof v === "bigint" ? (Number.isSafeInteger(Number(v)) ? Number(v) : v.toString()) : v);
    const objects = res.rows.map((row) => Object.fromEntries(scalarCols.map(({ c, i }) => [c.name, cell(row[i])])));
    const features = geomIdx >= 0
      ? res.rows.map((row, k) => ({ type: "Feature" as const, properties: objects[k], geometry: row[geomIdx] instanceof Uint8Array ? wkbToGeoJSON(row[geomIdx] as Uint8Array) : null }))
      : null;
    const digest = await resultDigest(res.rows, res.columns.map((c) => c.type));
    const t2 = performance.now();
    const sum = (f: (s: Scan) => number) => r.physical.scans.reduce((a, s) => a + f(s), 0);
    const m = (value: number | null, source: string): Metric<number> => ({ value, source });
    const metrics: Metrics = {
      elapsed_ms: res.metrics.elapsed_ms, rows_returned: res.metrics.rows_returned,
      row_groups_total: m(sum((s) => s.prune.total_row_groups), "source_observed"),
      row_groups_skipped: m(sum((s) => s.prune.skipped), "source_estimated"),
      rows_scanned_upper_bound: m(sum((s) => s.prune.kept_rows), "source_estimated"),
      estimated_bytes_upper_bound: m(sum((s) => s.prune.kept_bytes), "source_estimated"),
      bytes_read: res.metrics.bytes_read, peak_memory_bytes: m(null, "unknown"),
      notes: ["executed in the browser (DuckDB-Wasm); the server only planned and served authorized Parquet byte ranges", ...res.metrics.notes],
    };
    return {
      ...r,
      columns: res.columns.map((c, i) => ({ name: c.name, type: c.type, geometry: i === geomIdx })),
      rows: objects.slice(0, 1000), rows_total: res.rows.length,
      geojson: features ? { type: "FeatureCollection", features: features.filter((f) => f.geometry) as GeoJSON.Feature[] } : null,
      metrics, result_digest: digest,
      phases: res.phases ? { ...res.phases, boundary_ms: t2 - t1 } : undefined,
      timing: {
        total_ms: m(t2 - t0, "engine_observed"),
        plan_and_overhead_ms: m(Math.max(0, t1 - t0 - (res.metrics.elapsed_ms.value ?? 0)), "derived"),
        boundary_conversion_ms: m(t2 - t1, "engine_observed"),
      },
    };
  }

  /**
   * Browser execution can fail for reasons the planner cannot see from metadata. The failure is never hidden: it is
   * classified, reported (health, known incompatibilities), and the same request is re-planned with the browser disabled
   * (at most ONE fallback, none after a cancellation, no retry on the same engine); the response carries `fallback`.
   */
  async function placeWithFallback(first: RunResp, retry: (() => Promise<RunResp>) | null): Promise<RunResp> {
    const t0 = performance.now();
    try {
      const res = await resolvePlacement(first);
      if (first.placement === "browser") reportRun(first, true, res.rows_total, performance.now() - t0);
      return res;
    } catch (e) {
      const reason = e instanceof Error ? e.message : String(e);
      const cls = classifyFailure(reason);
      reportRun(first, false, reason, performance.now() - t0);
      if (!fallbackAllowed(cls)) throw e; // cancelled by the user: no fallback
      await wasm.recycle();
      if (!retry) {
        // browser-only deployment: there is no other engine. The failure is reported as what it is; nothing is retried or faked.
        deps.log(`browser engine failed (${cls}); this deployment has no other engine: ${reason}`, true);
        if (cls === "ENGINE_CRASH" || cls === "MEMORY") { st.probe = null; void probeRuntime(`after ${cls}`); }
        throw new LayerUnavailable(reason);
      }
      deps.log(`browser engine failed (${cls}), falling back once to the server engine: ${reason}`, true);
      // a crash/out-of-memory means the runtime we probed is gone: probe the new instance (in the background)
      if (cls === "ENGINE_CRASH" || cls === "MEMORY") { st.probe = null; void probeRuntime(`after ${cls}`); }
      const second = await retry();
      return { ...second, fallback: { from: first.physical.engine, reason, class: cls } };
    }
  }

  return { wasm, state: st, probeRuntime, ensureFreshProbe, bindSources, resolvePlacement, placeWithFallback, reportRun };
}

export type Placement = ReturnType<typeof createPlacement>;
