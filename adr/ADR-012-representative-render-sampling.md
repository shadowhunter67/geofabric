# ADR-012 — Representative render sampling: the map draws a sample of the viewport, never the first rows of a file

Status: Accepted (2026-10-17).

## Context

A viewport can hold more features than the map should draw (the buildings file has 87,000 in a district-sized view). The first versions cut the result at the first N rows (`LIMIT 20000`) and said "showing at most 20,000". The rows come in file order and the files are spatially sorted on purpose (that is what makes row-group pruning work), so the first 20,000 rows are one corner of the viewport. The picture lied about where the data is.

## Decision

1. **Rendering and analysis are separate.** The *query result* is the full logical result. Counts, aggregates and joins always see all of it. Only a row-returning request made *for drawing* may be thinned, and only when its answer reaches the draw budget.
2. **Thinning = `md5(id) mod m = 0`** (`Expr::HashBucket`), with `m` the smallest power of two such that `total / m <= budget`.
   - *Deterministic*: same data, viewport and budget give the same rows (no randomness).
   - *Spatially uniform in expectation*: the md5 of an identifier has no relation to position.
   - *Stable while panning*: `m` changes only when the count crosses a power of two.
   - *Nested*: the sample at `m` contains the one at `2m`.
   - *Engine-independent*: md5 is the same function in DuckDB and DuckDB-Wasm (`hash()` is not guaranteed to be), so the sample does not change when the planner moves a plan between the server and the browser. The browser lowering and the Rust lowering are covered by a shared golden fixture.
   Why not a grid with a quota per cell: it needs a window function (not in the IR), a viewport-dependent grid (the sample flickers while panning), and it over-represents empty-ish cells. Uniform thinning keeps the density picture honest: dense areas look dense.
3. **Protocol**: the client asks first as before. If the answer reaches the budget it asks the full count (`count` / `near_count`, never sampled) and, when the count exceeds the budget, repeats the request with `sample_modulus = m`. A view under the budget costs nothing extra.
4. **The gateway refuses `sample_modulus` for `count`, `count_by`, `join_count_by`, `near_count`** and for non-power-of-two values, and requires an `id` column. A sample cannot leak into an analysis.
5. **The UI says what it draws**: "Showing a representative sample of 10,918 / 87,342 features" (vi/en), in the legend and in the layer row. An exact-`LIMIT` mode stays available (checkbox in the Query tab) for people who want "the first N rows" and says so ("showing at most N").

## Consequences

- The shown count is between budget/2 and budget (power-of-two steps), not exactly the budget.
- Features in sparse areas appear with the same probability as in dense ones, so a very sparse area of a huge viewport may show a handful of features. That is a correct picture of the data.
- Two extra requests when capped (count, then the thinned query). Measured in the report.
- A source without an `id` column cannot be sampled (it falls back to the exact-limit message).

## Evidence

`crates/geo-gateway/tests/templates.rs` (four clusters: file-order cut shows one cluster, the thinned answer shows all four, deterministic, nested, refused for counts), `web/test/sampling.test.ts`, `web/e2e/sampling.spec.ts` (real buildings, drawn features come from all quadrants), golden lowering fixture `features_sampled_for_render`.

## Evidence update (2026-10-19)

Release gateway, real buildings (15.4 M rows), z14 viewport: capped query 0.6 s, count 0.13 s, thinned query 0.5–0.7 s; end to end in the browser 1.78 s to drawn (before: 2.24 s with the file-order cut), z12 2.0 s (before 2.26 s), z16 0.33 s (before 0.42 s). The drawn count is 12,234 and 13,259 of the viewport's features (between budget/2 and budget by construction). Server and browser engines draw the same sample (md5 is portable): digest equal on the 10 MB clustered file.
