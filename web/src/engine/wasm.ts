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

  /** The DuckDB-Wasm instance exists (a query now pays no instantiation cost). */
  isWarm(): boolean {
    return this.db !== null;
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
        // Range requests only: without this DuckDB-Wasm downloads the WHOLE file on first use (measured, ADR-009)
        await db.open({ filesystem: { reliableHeadRequests: true, allowFullHTTPReads: false, forceFullHTTPReads: false } });
        this.db = db;
        return db;
      })();
    }
    return this.starting;
  }

  /**
   * Executions are serialized: they share one DuckDB instance and register source files under fixed names
   * (`gf_src_N.parquet`), so a runtime probe running next to a user query would overwrite the other's files.
   */
  private queue: Promise<unknown> = Promise.resolve();
  private seq = 0;

  execute(plan: PlanGraph, ctx: ExecutionContext): Promise<ExecutionResult> {
    const run = this.queue.then(() => this.executeNow(plan, ctx));
    this.queue = run.catch(() => undefined);
    return run;
  }

  private async executeNow(plan: PlanGraph, ctx: ExecutionContext): Promise<ExecutionResult> {
    const t0 = performance.now();
    const compiled = compile(plan, ctx.sources, ctx.maxRows); // throws "unsupported"/"invalid" before any I/O
    // DuckDB caches Parquet metadata by file name: every execution registers its files under names never used before
    // (reusing `gf_src_0.parquet` for another file produced "No magic bytes found at end of file")
    const seq = ++this.seq;
    const files = compiled.files.map((f) => ({ ...f, name: f.name.replace("gf_src_", `gf${seq}_src_`) }));
    let sql = compiled.sql;
    compiled.files.forEach((f, i) => { sql = sql.split(`'${f.name}'`).join(`'${files[i].name}'`); });
    const est = ctx.estimatedInputBytes ?? 0;
    if (this.cumulativeInput > 0 && this.cumulativeInput + est > this.recycleBudget) await this.recycle();
    this.cumulativeInput += est;
    const tInit0 = performance.now();
    const db = await this.instance();
    const initMs = performance.now() - tInit0;
    const tOpen0 = performance.now();
    const buffered: string[] = [];
    for (const f of files) {
      try {
        await db.dropFile(f.name);
      } catch {
        /* not registered yet */
      }
      const url = new URL(f.url, location.href).href;
      if (f.access === "full") {
        // small file: one request for all of it (dozens of sequential range requests are slower than one download)
        const r = await fetch(url);
        if (!r.ok) throw new EngineError("engine", `fetching ${f.uri} failed: HTTP ${r.status}`);
        await db.registerFileBuffer(f.name, new Uint8Array(await r.arrayBuffer()));
        buffered.push(f.name);
      } else {
        db.registerFileURL(f.name, url, duckdb.DuckDBDataProtocol.HTTP, true);
      }
    }
    const conn = await db.connect();
    const openMs = performance.now() - tOpen0;
    let cancelled: "timeout" | "cancelled" | null = null;
    const cancel = (why: "timeout" | "cancelled") => {
      cancelled = why;
      void conn.cancelSent();
    };
    const timer = setTimeout(() => cancel("timeout"), ctx.timeoutMs);
    const onAbort = () => cancel("cancelled");
    ctx.signal?.addEventListener("abort", onAbort);
    try {
      const tFirst0 = performance.now();
      const reader = await conn.send(sql);
      const rows: unknown[][] = [];
      let columns: { name: string; type: string }[] = [];
      let firstMs = -1;
      let tDrain0 = 0;
      for await (const batch of reader) {
        if (firstMs < 0) { firstMs = performance.now() - tFirst0; tDrain0 = performance.now(); }
        if (cancelled) break;
        if (columns.length === 0) columns = batch.schema.fields.map((f) => ({ name: f.name, type: String(f.type) }));
        const vecs = columns.map((_, i) => batch.getChildAt(i)!);
        for (let r = 0; r < batch.numRows; r++) rows.push(vecs.map((v) => v.get(r)));
        if (rows.length > ctx.maxRows) throw new EngineError("limit", `result exceeds ${ctx.maxRows} rows`);
      }
      if (cancelled) throw new EngineError(cancelled, cancelled === "timeout" ? `browser query interrupted after ${ctx.timeoutMs} ms` : "browser query cancelled");
      const elapsed = performance.now() - t0;
      if (firstMs < 0) firstMs = performance.now() - tFirst0; // an empty result has no batch
      const drainMs = tDrain0 > 0 ? performance.now() - tDrain0 : 0;
      return {
        columns,
        rows,
        sql,
        phases: { init_ms: initMs, open_ms: openMs, first_ms: firstMs, drain_ms: drainMs, rows: rows.length },
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
      for (const n of [...buffered, ...files.map((f) => f.name)]) {
        try { await db.dropFile(n); } catch { /* ignore */ }
      }
    }
  }
}
