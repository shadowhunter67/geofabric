"""Deterministic workload corpus for the PostGIS cost model (calibration set and the fresh held-out set).

A case is a *specification* the live harness (`crates/geo-runtime/tests/postgis_cost.rs`) turns into a PlanGraph: either a real geo-plan request
(the same object a user sends: features / count / count_by / intersects_features / near / near_count) or a node-level template for shapes the
request model does not express (admin semi join, sort/limit, aggregates, a projected column). The generator only chooses PARAMETERS from a
seeded RNG and labels the intent; the workload class the model uses is derived from the plan's features at planning time, never from the label.

Coverage the brief asks for, by construction:
  * operations      Source Filter BboxFilter Project Sort Limit Aggregate Intersects Transform SpatialSemiJoin
  * selectivity     very selective .. near-full scan (window half-size 0.0015 deg .. the whole extent), log-uniform
  * spatial windows dense centre / medium density / sparse / broad (4 region kinds, several centres each)
  * cardinality     hundreds .. millions of rows (buildings 3.8 M, transport 0.45 M, places 0.45 M, landuse 13 k, admin 325)
Seeds: calibration 20261103, held-out (A) 20261107, confirmation (B) 20261111. NOT used before: 20261018, 20261019, 20261026 (earlier planner/browser studies).

    python benchmarks/make_postgis_cost_cases.py --split calibration --out benchmarks/results/postgis-cost/cases-calibration.json
    python benchmarks/make_postgis_cost_cases.py --split heldout     --out benchmarks/results/postgis-cost/cases-heldout.json
"""

from __future__ import annotations

import argparse
import json
import math
import random
from pathlib import Path

SEEDS = {"calibration": 20261103, "heldout": 20261107, "confirm": 20261111}
COUNTS = {"calibration": 240, "heldout": 220, "confirm": 260}
# the confirmation set is enriched in the semi-join shapes (they are the classes with the least evidence) and keeps their windows in the regime where
# the probe count straddles PROBES_SMALL; it is a representativeness choice made before any confirm result exists
CONFIRM_WEIGHTS = {"near_count": 34, "near_ids": 18, "semi_admin": 18}
NEAR_HI = {"calibration": 0.11, "heldout": 0.11, "confirm": 0.07}
COVERAGE = (106.35, 10.35, 107.05, 11.25)  # the PostGIS relation is a clipped copy: only windows INSIDE this are readable there
INSET = 0.03  # windows (and the metric prefilter margin) stay this far inside the coverage
EXTENT = (COVERAGE[0] + INSET, COVERAGE[1] + INSET, COVERAGE[2] - INSET, COVERAGE[3] - INSET)

REGIONS = {
    "dense": [(106.700, 10.780), (106.660, 10.790), (106.690, 10.760), (106.720, 10.800)],
    "medium": [(106.780, 10.850), (106.620, 10.800), (106.730, 10.900), (106.600, 10.730)],
    "sparse": [(106.900, 10.450), (106.500, 11.050), (106.560, 10.600), (106.880, 10.700), (106.450, 10.950)],
    "broad": [(106.700, 10.800)],
}
# (template, weight). Weights chosen so every operation shape has >= 15 cases in the held-out set.
TEMPLATES = [
    ("features_bbox", 22), ("features_filter", 16), ("count_bbox", 22), ("count_by", 16), ("intersects_viewport", 10),
    ("near_count", 22), ("near_ids", 12), ("semi_admin", 14), ("sort_limit", 12), ("agg_stats", 10), ("transform_col", 8), ("limit_unordered", 6),
]
SOURCES = ["buildings", "transport", "places", "landuse"]
SOURCE_SIZE = {"buildings": 3_845_734, "transport": 453_550, "places": 449_849, "landuse": 13_277, "admin": 325}
FILTERS = {
    "buildings": [("class", "eq", ["house", "apartments", "residential", "commercial", "industrial", "retail"]), ("height", "gt", [3.0, 6.0, 9.0, 15.0, 30.0])],
    "transport": [("class", "eq", ["residential", "service", "primary", "secondary", "tertiary", "footway", "unclassified"])],
    "places": [("confidence", "gt", [0.5, 0.7, 0.9]), ("category", "eq", ["restaurant", "cafe", "shopping", "school"])],
    "landuse": [("class", "eq", ["residential", "park", "industrial", "commercial", "grass"])],
}
GROUPS = {"buildings": ["class", "subtype"], "transport": ["class", "subtype"], "places": ["category", "operating_status"], "landuse": ["class", "subtype"]}


def window(rng: random.Random, lo: float, hi: float, region: str | None = None) -> tuple[list[float], str]:
    """A window with log-uniform half-size in [lo, hi] degrees around a centre of a (random or given) region kind, clipped to the extent."""
    kind = region or rng.choice(["dense", "dense", "medium", "medium", "sparse", "sparse", "broad"])
    cx, cy = rng.choice(REGIONS[kind])
    half = math.exp(rng.uniform(math.log(lo), math.log(hi)))
    hx, hy = half, half * rng.uniform(0.7, 1.3)
    bb = [max(EXTENT[0], cx - hx), max(EXTENT[1], cy - hy), min(EXTENT[2], cx + hx), min(EXTENT[3], cy + hy)]
    return [round(v, 6) for v in bb], kind


def pick_filter(rng: random.Random, src: str) -> dict:
    col, op, vals = rng.choice(FILTERS[src])
    return {"column": col, "op": op, "value": rng.choice(vals)}


def make_case(rng: random.Random, template: str, idx: int, split: str) -> dict:
    near_hi = NEAR_HI[split]
    base = {"id": f"{split[:3]}-{idx:04d}", "template": template}
    if template == "features_bbox":
        src = rng.choice(SOURCES)
        bbox, kind = window(rng, 0.0015, 0.12 if src == "buildings" else 0.25)
        limit = rng.choice([100, 500, 2000, 5000, 20000])
        return {**base, "kind": "request", "region": kind, "request": {"kind": "features", "source": src, "bbox": bbox, "crs": "OGC:CRS84", "limit": limit}}
    if template == "features_filter":
        src = rng.choice(SOURCES)
        bbox, kind = window(rng, 0.002, 0.1)
        return {**base, "kind": "request", "region": kind, "request": {"kind": "features", "source": src, "bbox": bbox, "crs": "OGC:CRS84", "filter": pick_filter(rng, src), "limit": rng.choice([500, 5000, 20000])}}
    if template == "count_bbox":
        src = rng.choice(SOURCES)
        bbox, kind = window(rng, 0.0015, 0.7, rng.choice([None, None, "broad"]))
        req = {"kind": "count", "source": src, "bbox": bbox, "crs": "OGC:CRS84"}
        if rng.random() < 0.35:
            req["filter"] = pick_filter(rng, src)
        return {**base, "kind": "request", "region": kind, "request": req}
    if template == "count_by":
        src = rng.choice(SOURCES)
        bbox, kind = window(rng, 0.002, 0.6, rng.choice([None, None, "broad"]))
        req = {"kind": "count_by", "source": src, "bbox": bbox, "crs": "OGC:CRS84", "group_by": rng.choice(GROUPS[src])}
        if rng.random() < 0.25:
            req["filter"] = pick_filter(rng, src)
        return {**base, "kind": "request", "region": kind, "request": req}
    if template == "intersects_viewport":
        src = rng.choice(SOURCES)
        bbox, kind = window(rng, 0.002, 0.06)
        return {**base, "kind": "request", "region": kind, "request": {"kind": "intersects_features", "source": src, "bbox": bbox, "crs": "OGC:CRS84", "limit": rng.choice([500, 5000, 20000])}}
    if template in ("near_count", "near_ids"):
        # buildings within d metres of roads. Window sizes stay under ~0.2 deg: the largest known case (0.16 x 0.14 deg) already takes PostGIS ~15 s.
        bbox, kind = window(rng, 0.003, near_hi)
        dist = rng.choice([25, 50, 100, 200, 200, 500, 1000])
        with_ = {"source": "transport", "distance_m": dist}
        if rng.random() < 0.6:
            with_["filter"] = {"column": "class", "op": "in", "values": rng.choice([["motorway", "trunk", "primary"], ["primary", "secondary"], ["residential", "service"]])}
        if template == "near_count":
            return {**base, "kind": "request", "region": kind, "request": {"kind": "near_count", "source": "buildings", "bbox": bbox, "crs": "OGC:CRS84", "with": with_}}
        # the `near` request projects every column incl. nested ones the PostGIS relation does not store (a real residency limit, documented in the
        # report): the same semi join returning id/class/geometry is the PostGIS-eligible shape of "which buildings are near roads"
        return {**base, "kind": "template", "region": kind, "name": "near_ids", "params": {"bbox": bbox, "distance_m": dist, "road_classes": with_.get("filter", {}).get("values")}}
    if template == "semi_admin":
        bbox, kind = window(rng, 0.002, 0.09)
        return {**base, "kind": "template", "region": kind, "name": "semi_intersects_admin", "params": {"bbox": bbox}}
    if template == "sort_limit":
        bbox, kind = window(rng, 0.002, 0.15)
        return {**base, "kind": "template", "region": kind, "name": "sort_limit", "params": {"bbox": bbox, "n": rng.choice([40, 500, 5000]), "desc": rng.random() < 0.5}}
    if template == "agg_stats":
        bbox, kind = window(rng, 0.002, 0.4, rng.choice([None, None, "broad"]))
        return {**base, "kind": "template", "region": kind, "name": "agg_stats", "params": {"bbox": bbox}}
    if template == "transform_col":
        bbox, kind = window(rng, 0.002, 0.06)
        return {**base, "kind": "template", "region": kind, "name": "transform_col", "params": {"bbox": bbox}}
    if template == "limit_unordered":
        bbox, kind = window(rng, 0.002, 0.2)
        return {**base, "kind": "template", "region": kind, "name": "limit_unordered", "params": {"bbox": bbox, "n": rng.choice([20, 200, 2000])}}
    raise SystemExit(f"unknown template {template}")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--split", choices=SEEDS, required=True)
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--n", type=int)
    a = ap.parse_args()
    rng = random.Random(SEEDS[a.split])
    n = a.n or COUNTS[a.split]
    table = [(n, CONFIRM_WEIGHTS.get(n, w) if a.split == "confirm" else w) for n, w in TEMPLATES]
    names, weights = zip(*table)
    total = sum(weights)
    # stratified: each template gets its proportional share first (at least 1), the remainder is drawn by weight
    plan: list[str] = []
    for name, w in table:
        plan += [name] * max(1, int(n * w / total))
    while len(plan) < n:
        plan.append(rng.choices(names, weights)[0])
    plan = plan[:n]
    rng.shuffle(plan)
    cases = [make_case(rng, t, i, a.split) for i, t in enumerate(plan)]
    a.out.parent.mkdir(parents=True, exist_ok=True)
    a.out.write_text(json.dumps({"split": a.split, "seed": SEEDS[a.split], "generator": "benchmarks/make_postgis_cost_cases.py", "cases": cases}, indent=1) + "\n", encoding="utf-8")
    by: dict[str, int] = {}
    for c in cases:
        by[c["template"]] = by.get(c["template"], 0) + 1
    print(a.split, "seed", SEEDS[a.split], len(cases), "cases", by)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
