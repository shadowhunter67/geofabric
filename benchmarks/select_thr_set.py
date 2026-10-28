"""Select the risk-threshold confirmation set from the heavy-regime pool, using PLAN-TIME information only.

The pool (`pool-thr_pool.json`, seed 20261301) is planned without executing anything (`plans-thr_pool.jsonl`). From the plan we know, for every case:
the probe count, the candidate pairs and the DuckDB timeout probability P (None below the regime where timeouts occur). The selection is stratified so that
(a) the borderline bands that discriminate between the candidate thresholds are well populated and (b) probes x candidate-pairs cells are all covered.
No measured time exists when this runs: nothing here can depend on how long a case takes.

    python benchmarks/select_thr_set.py --pool <pool.json> --plans <plans.jsonl> --out <cases.json> [--n 140] [--per-band 22]
"""

from __future__ import annotations

import argparse
import json
import random
import sys
from collections import defaultdict
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from postgis_cost_lib import load_raw  # noqa: E402

SEED = 20261302  # selection draw (the pool itself is seed 20261301)
BANDS = [("b1_0.08-0.12", 0.08, 0.12), ("b2_0.13-0.17", 0.13, 0.17), ("b3_0.18-0.22", 0.18, 0.22)]
PROBE_BINS = [("probes_10k-50k", 10_000, 50_000), ("probes_50k-250k", 50_000, 250_000), ("probes_gt250k", 250_000, 1e18)]
PAIR_BINS = [("pairs_1e7-3e7", 1e7, 3e7), ("pairs_3e7-7e7", 3e7, 7e7), ("pairs_7e7-2e8", 7e7, 2e8), ("pairs_gt2e8", 2e8, 1e30)]


def info(rec: dict) -> dict | None:
    pl = rec["planner"]
    sj = pl["workload"].get("semi_join")
    if not sj or sj.get("predicate") != "within_distance" or sj.get("candidate_pairs") is None:
        return None
    duck = next((c for c in pl["candidates"] if c["engine"] == "duckdb"), None)
    risk = ((duck or {}).get("cost_model") or {}).get("risk") or {}
    p = risk.get("timeout_probability")
    if p is None or sj["candidate_pairs"] < 1e7:
        return None
    return {"id": rec["id"], "probes": sj["expected_probes"], "pairs": sj["candidate_pairs"], "p": p, "distance_m": sj["distance_m"]}


def binof(v: float, bins) -> str | None:
    return next((n for n, lo, hi in bins if lo <= v < hi), None)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--pool", type=Path, nargs="+", required=True)
    ap.add_argument("--plans", type=Path, nargs="+", required=True)
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--n", type=int, default=140)
    ap.add_argument("--per-band", type=int, default=22)
    a = ap.parse_args()
    rng = random.Random(SEED)
    pools = [json.loads(p.read_text(encoding="utf-8")) for p in a.pool]
    cases = {c["id"]: c for pool in pools for c in pool["cases"]}
    items = [x for p in a.plans for x in (info(r) for r in load_raw(p)) if x]
    for x in items:
        x["band"] = next((n for n, lo, hi in BANDS if lo <= x["p"] < hi), None)
        x["pb"], x["qb"] = binof(x["probes"], PROBE_BINS), binof(x["pairs"], PAIR_BINS)
    heavy = [x for x in items if x["pb"] and x["qb"]]
    chosen: dict[str, dict] = {}
    # (a) the borderline bands, balanced across probe bins inside each band
    for name, _, _ in BANDS:
        cand = [x for x in heavy if x["band"] == name]
        rng.shuffle(cand)
        by = defaultdict(list)
        for x in cand:
            by[x["pb"]].append(x)
        k = 0
        picked = 0
        keys = sorted(by)
        while picked < a.per_band and any(by.values()):
            lst = by[keys[k % len(keys)]]
            if lst:
                x = lst.pop()
                chosen[x["id"]] = x
                picked += 1
            k += 1
    # (b) fill up to n, round-robin over the probes x pairs cells (every case outside the bands; in-band cases are already counted)
    cells = defaultdict(list)
    for x in heavy:
        if x["id"] not in chosen:
            cells[(x["pb"], x["qb"])].append(x)
    for lst in cells.values():
        rng.shuffle(lst)
    keys = sorted(cells)
    while len(chosen) < a.n and any(cells.values()):
        for k in keys:
            if cells[k] and len(chosen) < a.n:
                x = cells[k].pop()
                chosen[x["id"]] = x
    sel = sorted(chosen.values(), key=lambda x: x["id"])
    out = {"split": "thr_heavy", "pool_seeds": [p["seed"] for p in pools], "selection_seed": SEED, "generator": "benchmarks/select_thr_set.py",
           "selected_from": {"pool_cases": len(cases), "heavy_regime_in_pool": len(heavy)},
           "cases": [dict(cases[x["id"]], id=x["id"]) for x in sel]}
    a.out.write_text(json.dumps(out, indent=1) + "\n", encoding="utf-8", newline="\n")
    cnt = lambda key: {k: sum(1 for x in sel if x[key] == k) for k in sorted({x[key] for x in sel if x[key]}, key=str)}
    print(len(sel), "selected from", len(heavy), "heavy-regime pool cases (pool", len(cases), ")")
    print("bands", cnt("band"), "| other", sum(1 for x in sel if not x["band"]))
    print("probes", cnt("pb"))
    print("pairs", cnt("qb"))
    print("radius", cnt("distance_m"))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
