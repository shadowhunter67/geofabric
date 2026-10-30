"""Select the result-risk-policy confirmation set from the plan-only pool (seed 20261501) using PLAN-TIME information only.

Strata: (a) DuckDB timeout-probability bands 0.08-0.12 / 0.12-0.18 / 0.18-0.22, balanced across result shapes; (b) the estimated-result-row boundary
(500k-800k, 800k-950k, 950k-1M planned; 1.0M-1.05M, 1.05M-1.2M, 1.2M+ refused by result-policy-v1); (c) a round-robin fill over shapes among the
planned heavy-regime cases. No measured time exists when this runs.

    python benchmarks/select_rp_set.py --pool <pool.json> --plans <plans.jsonl> --out <cases.json> [--n 156]
"""
from __future__ import annotations
import argparse, json, random, re, sys
from collections import defaultdict
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from postgis_cost_lib import load_raw  # noqa: E402

SEED = 20261502
P_BANDS = [("P_0.08-0.12", 0.08, 0.12), ("P_0.12-0.18", 0.12, 0.18), ("P_0.18-0.22", 0.18, 0.22)]
ROW_BANDS = [("rows_500k-800k", 5e5, 8e5, 15), ("rows_800k-950k", 8e5, 9.5e5, 15), ("rows_950k-1M", 9.5e5, 1e6 + 1, 15),
             ("refused_1.0M-1.05M", 1e6 + 1, 1.05e6, 10), ("refused_1.05M-1.2M", 1.05e6, 1.2e6, 14), ("refused_1.2M+", 1.2e6, 1e18, 10)]


def info(rec, case):
    pl = rec["planner"]
    shape = case.get("shape") or case["template"]
    if "error" in pl:
        m = re.search(r"estimated at (\d+) rows \(up to (\d+)\)", pl["error"])
        return {"id": rec["id"], "shape": shape, "refused": True, "rows": int(m.group(1)) if m else None, "p": None}
    duck = next((c for c in pl["candidates"] if c["engine"] == "duckdb"), None)
    p = (((duck or {}).get("cost_model") or {}).get("risk") or {}).get("timeout_probability")
    sj = pl["workload"].get("semi_join") or {}
    return {"id": rec["id"], "shape": shape, "refused": False, "rows": pl["estimate_rows"], "p": p, "pairs": sj.get("candidate_pairs")}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--pool", type=Path, required=True)
    ap.add_argument("--plans", type=Path, required=True)
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--n", type=int, default=156)
    a = ap.parse_args()
    rng = random.Random(SEED)
    pool = json.loads(a.pool.read_text(encoding="utf-8"))
    cases = {c["id"]: c for c in pool["cases"]}
    items = [info(r, cases[r["id"]]) for r in load_raw(a.plans)]
    chosen: dict[str, dict] = {}

    def take(cands, k, tag):
        by = defaultdict(list)
        for x in cands:
            if x["id"] not in chosen:
                by[x["shape"]].append(x)
        for v in by.values():
            rng.shuffle(v)
        keys = sorted(by)
        got = 0
        while got < k and any(by.values()):
            for s in keys:
                if by[s] and got < k:
                    x = by[s].pop()
                    x["stratum"] = tag
                    chosen[x["id"]] = x
                    got += 1

    for name, lo, hi in P_BANDS:
        take([x for x in items if not x["refused"] and x["p"] is not None and lo <= x["p"] < hi], 20, name)
    for name, lo, hi, k in ROW_BANDS:
        take([x for x in items if x["rows"] is not None and lo <= x["rows"] < hi and x["shape"] in ("raw_geom", "raw_ids") and x["refused"] == name.startswith("refused")], k, name)
    take([x for x in items if not x["refused"] and x["p"] is not None], max(0, a.n - len(chosen)), "heavy_fill")
    sel = sorted(chosen.values(), key=lambda x: x["id"])
    out = {"split": "rp_heavy", "pool_seed": pool["seed"], "selection_seed": SEED, "generator": "benchmarks/select_rp_set.py",
           "selected_from": {"pool_cases": len(cases)}, "cases": [dict(cases[x["id"]], id=x["id"]) for x in sel]}
    a.out.write_text(json.dumps(out, indent=1) + "\n", encoding="utf-8", newline="\n")
    cnt = lambda key: {k: sum(1 for x in sel if x[key] == k) for k in sorted({x[key] for x in sel})}
    print(len(sel), "selected | strata", cnt("stratum"), "| shapes", cnt("shape"), "| refused", sum(1 for x in sel if x["refused"]))
    (a.out.parent / (a.out.stem.replace("cases-", "") + "-frozen-predictions.json")).write_text(json.dumps(sel, indent=1) + "\n", encoding="utf-8", newline="\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
