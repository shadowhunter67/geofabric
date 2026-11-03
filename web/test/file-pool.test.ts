import { test } from "node:test";
import assert from "node:assert/strict";
import { FilePool } from "../src/engine/file-pool.ts";

const bytes = (n: number) => new ArrayBuffer(n);

test("a file is downloaded once; later and concurrent requests share it", async () => {
  let calls = 0;
  const pool = new FilePool(1000, async () => { calls++; await new Promise((r) => setTimeout(r, 5)); return bytes(100); });
  const [a, b] = await Promise.all([pool.get("u"), pool.get("u")]);
  const c = await pool.get("u");
  assert.equal(calls, 1);
  assert.equal(pool.downloads, 1);
  assert.strictEqual(a, b);
  assert.strictEqual(a, c);
});

test("a failed download is not cached: the next request tries again", async () => {
  let calls = 0;
  const pool = new FilePool(1000, async () => { calls++; if (calls === 1) throw new Error("HTTP 503"); return bytes(10); });
  await assert.rejects(pool.get("u"), /503/);
  const ok = await pool.get("u");
  assert.equal(ok.byteLength, 10);
  assert.equal(calls, 2);
});

test("the oldest files are evicted beyond the budget; the newest stays", async () => {
  const pool = new FilePool(250, async () => bytes(100));
  await pool.get("a"); await pool.get("b"); await pool.get("c");
  assert.ok(pool.heldBytes <= 250, `held ${pool.heldBytes}`);
  const before = pool.downloads;
  await pool.get("c");
  assert.equal(pool.downloads, before, "the newest file is still pooled");
  await pool.get("a");
  assert.equal(pool.downloads, before + 1, "the oldest was evicted and is downloaded again");
});

test("a file bigger than the whole budget is served but not kept", async () => {
  const pool = new FilePool(50, async () => bytes(100));
  const buf = await pool.get("big");
  assert.equal(buf.byteLength, 100);
  assert.equal(pool.heldBytes, 0);
  await pool.get("big");
  assert.equal(pool.downloads, 2);
});

test("one aborted waiter does not cancel a download another waiter still needs", async () => {
  let calls = 0;
  let underlyingSignal: AbortSignal | undefined;
  let finish!: (value: ArrayBuffer) => void;
  const pool = new FilePool(1000, async (_url, signal) => {
    calls++;
    underlyingSignal = signal;
    return new Promise<ArrayBuffer>((resolve) => { finish = resolve; });
  });
  const firstController = new AbortController();
  const first = pool.get("shared", firstController.signal);
  const second = pool.get("shared");
  await Promise.resolve();
  firstController.abort();
  await assert.rejects(first, (error: unknown) => error instanceof Error && error.name === "AbortError");
  assert.equal(underlyingSignal?.aborted, false);
  finish(bytes(12));
  const received = await second;
  assert.equal(received.byteLength, 12);
  assert.strictEqual(await pool.get("shared"), received, "completed bytes remain cached");
  assert.equal(calls, 1);
});

test("the last aborted waiter cancels the fetch, removes it, and a later get refetches", async () => {
  let calls = 0;
  let firstSignal: AbortSignal | undefined;
  const pool = new FilePool(1000, async (_url, signal) => {
    calls++;
    if (calls > 1) return bytes(9);
    firstSignal = signal;
    return new Promise<ArrayBuffer>((_resolve, reject) => {
      if (signal?.aborted) reject(new DOMException("stopped", "AbortError"));
      else signal?.addEventListener("abort", () => reject(new DOMException("stopped", "AbortError")), { once: true });
    });
  });
  const controller = new AbortController();
  const waiting = pool.get("region", controller.signal);
  await Promise.resolve();
  controller.abort();
  await assert.rejects(waiting, (error: unknown) => error instanceof Error && error.name === "AbortError");
  assert.equal(firstSignal?.aborted, true);
  assert.equal(pool.heldBytes, 0);
  const retry = await pool.get("region");
  assert.equal(retry.byteLength, 9);
  assert.equal(calls, 2);
  assert.strictEqual(await pool.get("region"), retry, "returning to the region uses the immutable cache");
  assert.equal(calls, 2);
});

test("an abort after bytes arrive cancels that caller but keeps the completed bytes cached", async () => {
  let calls = 0;
  let finish!: (value: ArrayBuffer) => void;
  const pool = new FilePool(1000, async () => {
    calls++;
    return new Promise<ArrayBuffer>((resolve) => { finish = resolve; });
  });
  const controller = new AbortController();
  const waiting = pool.get("arrived", controller.signal);
  const arrived = bytes(7);
  finish(arrived);
  controller.abort();
  await assert.rejects(waiting, (error: unknown) => error instanceof Error && error.name === "AbortError");
  assert.strictEqual(await pool.get("arrived"), arrived);
  assert.equal(calls, 1);
});
