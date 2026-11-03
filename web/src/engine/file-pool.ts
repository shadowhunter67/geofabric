// Whole-file downloads for the browser engine's "full" access mode, shared by every query of the page session.
//
// Without it each query fetched the same GeoParquet file again (the first view of the public demo downloaded the 13.6 MB buildings file four
// times). The HTTP cache cannot be relied on: it is bypassed by "disable cache", by some privacy modes and by Safari policies, and concurrent
// queries would all miss it. The pool keeps the bytes in memory, de-duplicates concurrent requests for one URL, never caches a failure, and
// evicts the oldest files beyond `maxBytes` (one file larger than the budget is still served, just not kept).
// No DOM, no DuckDB: unit-tested under node.

export type FetchBytes = (url: string, signal?: AbortSignal) => Promise<ArrayBuffer>;

interface PoolEntry {
  promise: Promise<ArrayBuffer>;
  controller: AbortController;
  waiters: number;
  settled: boolean;
  abandoned: boolean;
}

function abortError(): DOMException {
  return new DOMException("The operation was aborted", "AbortError");
}

export class FilePool {
  private files = new Map<string, PoolEntry>();
  private sizes = new Map<string, number>();
  /** number of downloads actually started (for tests and the Developer view) */
  downloads = 0;

  private readonly maxBytes: number;
  private readonly fetchBytes: FetchBytes;

  // (no TS parameter properties: the module must also load under node's type-stripping test runner)
  constructor(maxBytes: number, fetchBytes: FetchBytes) {
    this.maxBytes = maxBytes;
    this.fetchBytes = fetchBytes;
  }

  /** The bytes of `url`. The returned buffer is shared: callers that hand it to something that takes ownership must copy it. */
  get(url: string, signal?: AbortSignal): Promise<ArrayBuffer> {
    if (signal?.aborted) return Promise.reject(abortError());
    let entry = this.files.get(url);
    if (entry) return this.waitFor(url, entry, signal);
    this.downloads++;
    const controller = new AbortController();
    entry = { promise: Promise.resolve(new ArrayBuffer(0)), controller, waiters: 0, settled: false, abandoned: false };
    let download: Promise<ArrayBuffer>;
    try {
      download = Promise.resolve(this.fetchBytes(url, controller.signal));
    } catch (err) {
      download = Promise.reject(err);
    }
    entry.promise = download.then(
      (buf) => {
        entry!.settled = true;
        if (!entry!.abandoned && this.files.get(url) === entry) {
          this.sizes.set(url, buf.byteLength);
          this.evict(url);
        }
        return buf;
      },
      (err) => {
        entry!.settled = true;
        if (this.files.get(url) === entry) this.files.delete(url); // a failure is never cached
        this.sizes.delete(url);
        throw err;
      },
    );
    this.files.set(url, entry);
    return this.waitFor(url, entry, signal);
  }

  /** Bytes currently held (finished downloads only). */
  get heldBytes(): number {
    let n = 0;
    for (const s of this.sizes.values()) n += s;
    return n;
  }

  clear(): void {
    this.files.clear();
    this.sizes.clear();
  }

  private waitFor(url: string, entry: PoolEntry, signal?: AbortSignal): Promise<ArrayBuffer> {
    entry.waiters++;
    return new Promise((resolve, reject) => {
      let done = false;
      const release = () => {
        entry.waiters--;
        signal?.removeEventListener("abort", onAbort);
      };
      const onAbort = () => {
        if (done) return;
        done = true;
        release();
        // Let an already-resolved download's reaction run first: the caller is still cancelled,
        // but immutable bytes that arrived before the abort remain reusable in the cache.
        setTimeout(() => {
          if (entry.waiters === 0 && !entry.settled) {
            entry.abandoned = true;
            if (this.files.get(url) === entry) this.files.delete(url);
            this.sizes.delete(url);
            entry.controller.abort();
          }
          reject(abortError());
        }, 0);
      };
      signal?.addEventListener("abort", onAbort, { once: true });
      entry.promise.then(
        (buf) => {
          if (done) return;
          done = true;
          release();
          resolve(buf);
        },
        (err: unknown) => {
          if (done) return;
          done = true;
          release();
          reject(err);
        },
      );
    });
  }

  private evict(keep: string): void {
    let total = this.heldBytes;
    for (const url of [...this.files.keys()]) {
      if (total <= this.maxBytes) break;
      if (url === keep && this.files.size === 1) break;
      const size = this.sizes.get(url);
      if (size === undefined) continue; // still downloading: not counted, not evictable
      this.files.delete(url);
      this.sizes.delete(url);
      total -= size;
    }
    // a single file bigger than the whole budget is served but not kept
    if (this.heldBytes > this.maxBytes) {
      this.files.delete(keep);
      this.sizes.delete(keep);
    }
  }
}
