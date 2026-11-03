import type { ExecutionResult, ShardBinding } from "./contract.ts";

function abortError(): DOMException {
  return new DOMException("The operation was aborted", "AbortError");
}

function sqlString(value: string): string {
  return `'${value.replaceAll("'", "''")}'`;
}

/** A physical shard name unique to one execution and source. */
export function shardFileName(executionPrefix: string, sourceIndex: number, shardIndex: number): string {
  return `${executionPrefix}_src_${sourceIndex}_s${shardIndex}.parquet`;
}

/** Replace one logical Parquet literal with the physical files DuckDB should scan. */
export function expandSourceSql(sql: string, logicalName: string, shardNames: string[]): string {
  const logicalLiteral = sqlString(logicalName);
  if (!sql.includes(logicalLiteral)) throw new Error(`logical source ${logicalName} is absent from the lowered SQL`);
  const physicalList = `[${shardNames.map(sqlString).join(",")}]`;
  return sql.split(logicalLiteral).join(physicalList);
}

/** Run promise work with bounded concurrency, retaining one settled result per input item. */
export function mapLimit<T, R>(
  items: T[],
  limit: number,
  fn: (item: T, index: number) => Promise<R>,
  signal?: AbortSignal,
): Promise<PromiseSettledResult<R>[]> {
  if (items.length === 0) return Promise.resolve([]);
  const concurrency = Number.isFinite(limit) && limit > 0 ? Math.max(1, Math.floor(limit)) : 1;

  return new Promise((resolve) => {
    const results = new Array<PromiseSettledResult<R>>(items.length);
    let next = 0;
    let active = 0;
    let settled = 0;

    const finishAborted = () => {
      while (next < items.length) {
        results[next++] = { status: "rejected", reason: abortError() };
        settled++;
      }
    };

    const schedule = () => {
      if (signal?.aborted) finishAborted();
      while (!signal?.aborted && active < concurrency && next < items.length) {
        const index = next++;
        active++;
        Promise.resolve()
          .then(() => {
            if (signal?.aborted) throw abortError();
            return fn(items[index], index);
          })
          .then(
            (value) => { results[index] = { status: "fulfilled", value }; },
            (reason: unknown) => { results[index] = { status: "rejected", reason }; },
          )
          .finally(() => {
            active--;
            settled++;
            schedule();
          });
      }
      if (settled === items.length) {
        signal?.removeEventListener("abort", onAbort);
        resolve(results);
      }
    };

    const onAbort = () => schedule();
    signal?.addEventListener("abort", onAbort, { once: true });
    schedule();
  });
}

function failureReason(reason: unknown): string {
  return reason instanceof Error ? reason.message : String(reason);
}

/** Download the whole-file shards; range shards are registered directly by the engine. */
export async function loadShards(
  shards: ShardBinding[],
  deps: { getBytes(url: string, signal?: AbortSignal): Promise<ArrayBuffer>; concurrency: number; signal?: AbortSignal },
): Promise<{ loaded: { shard: ShardBinding; bytes: ArrayBuffer }[]; failed: { shard: ShardBinding; reason: string }[] }> {
  const fullShards = shards.filter((shard) => shard.access === "full");
  const settled = await mapLimit(fullShards, deps.concurrency, (shard) => deps.getBytes(shard.url, deps.signal), deps.signal);
  const loaded: { shard: ShardBinding; bytes: ArrayBuffer }[] = [];
  const failed: { shard: ShardBinding; reason: string }[] = [];
  settled.forEach((result, index) => {
    const shard = fullShards[index];
    if (result.status === "fulfilled") loaded.push({ shard, bytes: result.value });
    else failed.push({ shard, reason: failureReason(result.reason) });
  });
  return { loaded, failed };
}

export function summarizeIncomplete(
  failed: { shard: ShardBinding; reason: string }[],
  total: number,
): ExecutionResult["incomplete"] | undefined {
  if (failed.length === 0) return undefined;
  return { failed_shards: failed.map(({ shard, reason }) => ({ id: shard.id, reason })), total_shards: total };
}
