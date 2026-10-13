// Browser execution engine: DuckDB-Wasm behind the same contract as the server engine.
// The UI never talks to DuckDB-Wasm directly: it asks the planner (gateway) where to run a plan and,
// when the answer is "browser", hands the PlanGraph to this class.

import * as duckdb from "@duckdb/duckdb-wasm";
import mvpWasm from "@duckdb/duckdb-wasm/dist/duckdb-mvp.wasm?url";
import mvpWorker from "@duckdb/duckdb-wasm/dist/duckdb-browser-mvp.worker.js?url";
import ehWasm from "@duckdb/duckdb-wasm/dist/duckdb-eh.wasm?url";
import ehWorker from "@duckdb/duckdb-wasm/dist/duckdb-browser-eh.worker.js?url";
import { compile, SUPPORTED_OPS } from "./sqlgen.ts";
import { EngineError } from "./contract.ts";
import type { Capabilities, ExecutionContext, ExecutionEngine, ExecutionResult, PlanGraph } from "./contract.ts";

export const ENGINE_ID = "duckdb-wasm";

const BUNDLES: duckdb.DuckDBBundles = {
  mvp: { mainModule: mvpWasm, mainWorker: mvpWorker },
  eh: { mainModule: ehWasm, mainWorker: ehWorker },
};

/**
 * A long-lived DuckDB-Wasm instance accumulates file metadata and cached ranges. Measured on real
 * data: after queries over several large files in one tab the tab crashed ("Target crashed"), while
 * the very same query in a fresh page worked. So the instance is recycled (worker terminated,
 * new one created) once the cumulative planner-estimated input exceeds this budget.
 */
export const RECYCLE_AFTER_INPUT_BYTES = 192 * 1024 * 1024;

export class DuckDbWasmEngine implements ExecutionEngine {
  private db: duckdb.AsyncDuckDB | null = null;
  private starting: Promise<duckdb.AsyncDuckDB> | null = null;
  private cumulativeInput = 0;
  recycles = 0;
  /** Cumulative estimated input after which the instance is recycled (tests/benchmarks may change it). */
  recycleBudget = RECYCLE_AFTER_INPUT_BYTES;

  /** Terminate the worker and forget the instance (next query starts a fresh one). */
  async recycle(): Promise<void> {
    const db = this.db ?? (await this.starting?.catch(() => null)) ?? null;
    this.db = null;
    this.starting = null;
    this.cumulativeInput = 0;
    this.recycles++;
    try {
      await db?.terminate();
    } catch {
      /* worker already gone */
    }
  }

  id(): string {
    return ENGINE_ID;
  }

  capabilities(): Capabilities {
    return { id: ENGINE_ID, kind: "browser", ops: SUPPORTED_OPS };
  }

  async instance(): Promise<duckdb.AsyncDuckDB> {
    if (this.db) return this.db;
    if (!this.starting) {
      this.starting = (async () => {
        const bundle = await duckdb.selectBundle(BUNDLES);
        const worker = new Worker(bundle.mainWorker!);
        const db = new duckdb.AsyncDuckDB(new duckdb.ConsoleLogger(duckdb.LogLevel.WARNING), worker);
        await db.instantiate(bundle.mainModule, bundle.pthreadWorker);
        this.db = db;
        return db;
      })();
    }
    return this.starting;
  }

  async execute(plan: PlanGraph, ctx: ExecutionContext): Promise<ExecutionResult> {
    const t0 = performance.now();
    const { sql, files } = compile(plan, ctx.sources, ctx.maxRows); // throws "unsupported"/"invalid" before any I/O
    const est = ctx.estimatedInputBytes ?? 0;
    if (this.cumulativeInput > 0 && this.cumulativeInput + est > this.recycleBudget) await this.recycle();
    this.cumulativeInput += est;
    const db = await this.instance();
    for (const f of files) {
      try {
        await db.dropFile(f.name);
      } catch {
        /* not registered yet */
      }
      db.registerFileURL(f.name, new URL(f.url, location.href).href, duckdb.DuckDBDataProtocol.HTTP, false);
    }
    const conn = await db.connect();
    let cancelled: "timeout" | "cancelled" | null = null;
    const cancel = (why: "timeout" | "cancelled") => {
      cancelled = why;
      void conn.cancelSent();
    };
    const timer = setTimeout(() => cancel("timeout"), ctx.timeoutMs);
    const onAbort = () => cancel("cancelled");
    ctx.signal?.addEventListener("abort", onAbort);
    try {
      const reader = await conn.send(sql);
      const rows: unknown[][] = [];
      let columns: { name: string; type: string }[] = [];
      for await (const batch of reader) {
        if (cancelled) break;
        if (columns.length === 0) columns = batch.schema.fields.map((f) => ({ name: f.name, type: String(f.type) }));
        const vecs = columns.map((_, i) => batch.getChildAt(i)!);
        for (let r = 0; r < batch.numRows; r++) rows.push(vecs.map((v) => v.get(r)));
        if (rows.length > ctx.maxRows) throw new EngineError("limit", `result exceeds ${ctx.maxRows} rows`);
      }
      if (cancelled) throw new EngineError(cancelled, cancelled === "timeout" ? `browser query interrupted after ${ctx.timeoutMs} ms` : "browser query cancelled");
      const elapsed = performance.now() - t0;
      return {
        columns,
        rows,
        sql,
        metrics: {
          elapsed_ms: { value: elapsed, source: "engine_observed" },
          rows_returned: { value: rows.length, source: "engine_observed" },
          bytes_read: { value: null, source: "unknown" },
          notes: ["bytes read over the network is not measured by the browser engine"],
        },
      };
    } catch (e) {
      if (e instanceof EngineError) throw e;
      if (cancelled) throw new EngineError(cancelled, cancelled === "timeout" ? `browser query interrupted after ${ctx.timeoutMs} ms` : "browser query cancelled");
      throw new EngineError("engine", e instanceof Error ? e.message : String(e));
    } finally {
      clearTimeout(timer);
      ctx.signal?.removeEventListener("abort", onAbort);
      await conn.close();
    }
  }
}
