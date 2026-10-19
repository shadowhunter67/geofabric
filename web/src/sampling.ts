/**
 * Representative rendering sample (ADR-012).
 *
 * A viewport can hold more features than the map should draw. Cutting the result at the first N rows of the file draws one corner
 * of the data (files are spatially sorted), which is not a picture of the viewport. Instead the result is THINNED: only the rows
 * with `md5(id) mod m = 0` are kept, where m is a power of two chosen so that about `budget` rows or fewer remain.
 *
 * Properties: deterministic (same data, viewport and budget give the same rows, no randomness), spatially uniform in expectation
 * (md5 of an id has nothing to do with where the feature is), stable while panning (m changes only when the count crosses a power
 * of two) and nested (the sample at m contains the one at 2m).
 *
 * Sampling is a RENDERING decision. Counts, aggregates and joins never see it: the full logical count is asked for separately.
 */

/** Default number of features the map draws for the active result. */
export const RENDER_CAP = 20000;

/** Smallest power of two m such that total / m <= budget (1 when everything fits). */
export function sampleModulus(total: number, budget: number): number {
  if (!(budget >= 1) || !(total > budget)) return 1;
  let m = 1;
  while (total / m > budget && m < 1 << 20) m *= 2;
  return m;
}

export interface Sampling {
  /** features in the viewport that satisfy the query (the full logical result) */
  total: number;
  /** features actually returned for drawing */
  shown: number;
  modulus: number;
  budget: number;
}

export interface SampleResult<R> { result: R; sampling: Sampling | null }

/**
 * Given the first (capped) answer of a row-returning query, decide whether it is a sample-worthy view and, if so, fetch the
 * representative one. `count` answers the full logical count; `fetchSampled` re-runs the same query thinned by `modulus`.
 * The first answer is kept when everything fits (a result that merely equals the budget is not "capped").
 */
export async function sampleIfCapped<R extends { rows_total: number }>(
  first: R,
  budget: number,
  count: () => Promise<number>,
  fetchSampled: (modulus: number) => Promise<R>,
): Promise<SampleResult<R>> {
  if (first.rows_total < budget) return { result: first, sampling: null };
  const total = await count();
  if (!(total > budget)) return { result: first, sampling: null };
  const modulus = sampleModulus(total, budget);
  const sampled = await fetchSampled(modulus);
  return { result: sampled, sampling: { total, shown: sampled.rows_total, modulus, budget } };
}
