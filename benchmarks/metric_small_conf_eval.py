"""metric_small confirmation: the 256-cell histogram (benchmark-time grids over the planner's own extent) vs the shipped 128-cell one, on a FRESH set.

Everything is decided BEFORE looking at the confirmation set: the time model is [ln_probes, ln_right] fitted on ms_disc + ms_cal only, the estimators are the
centre-count grids of `metric_small_truth.py grids`, the truth is PostgreSQL (`truth-ms_conf.json`). Nothing here is wired into the planner (no automatic promotion).

    python benchmarks/metric_small_conf_eval.py --out metric-small-conf-eval.json
"""

from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path

import numpy as np

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import metric_small_models as M  # noqa: E402
from metric_semijoin_discovery import expand, spec_bbox_dist  # noqa: E402
from metric_semijoin_truth import classes  # noqa: E402
from postgis_cost_lib import load_raw  # noqa: E402

R = M.R
M.TR.update(json.loads((R / "truth-ms_conf.json").read_text()))
G = np.load(R / "raw" / "grids.npz")


def est(table: str, n: int, cls, w) -> float:
    ix0, iy0, cx, cy = G[f"origin|{n}"]
    if table == "buildings":
        a = G[f"buildings|{n}|all"].astype(float)
    else:
        a = sum(G[k].astype(float) for k in G.files if k.startswith(f"transport|{n}|") and (not cls or k.split("|")[2] in cls))
    x0, y0 = 104.999 + ix0 * cx, 8.607 + iy0 * cy
    fx = np.clip(np.minimum((np.arange(a.shape[1]) + 1) * cx + x0, w[2]) - np.maximum(np.arange(a.shape[1]) * cx + x0, w[0]), 0, None) / cx
    fy = np.clip(np.minimum((np.arange(a.shape[0]) + 1) * cy + y0, w[3]) - np.maximum(np.arange(a.shape[0]) * cy + y0, w[1]), 0, None) / cy
    return float(fy @ a @ fx)


def pct(x, p):
    return float(np.percentile(x, p))


def stats(x):
    x = np.asarray(x, float)
    return {"n": len(x), "median": round(float(np.median(x)), 3), "p90": round(pct(x, 90), 3), "p95": round(pct(x, 95), 3), "max": round(float(x.max()), 2)}


def with_grid(split: str, n: int | None):
    out = []
    by = {r["id"]: r for r in M.load(split)}
    for r in load_raw(R / "raw" / f"{split}.jsonl"):
        if r["id"] not in by:
            continue
        d = dict(by[r["id"]])
        if n is not None:
            sd = spec_bbox_dist(r["spec"])
            if not sd:
                continue
            d["probes"] = max(est("buildings", n, None, sd[0]), 1)
            d["right"] = max(est("transport", n, classes(r["spec"]), expand(sd[0], sd[1])), 1)
        out.append(d)
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    res: dict = {"set": "ms_conf (seed 20261409)", "time_model": "[ln_probes, ln_right], fitted on ms_disc + ms_cal only"}
    C = M.load("ms_conf")
    res["usable_conf"] = len(C)
    arr = lambda rows, k: np.array([r[k] for r in rows], float)
    res["estimate_error"] = {"current_128_planner": {"left": stats(M.err(arr(C, "probes"), arr(C, "t_left"))), "right": stats(M.err(arr(C, "right"), arr(C, "t_right"))),
                                                      "pairs": stats(M.err(arr(C, "pairs"), arr(C, "t_pairs")))}}
    ids = {r["id"] for r in C}
    rows = []
    for r in load_raw(R / "raw" / "ms_conf.jsonl"):
        k = f"ms_conf:{r['id']}"
        sd = spec_bbox_dist(r["spec"])
        if r["id"] in ids and sd:
            rows.append((M.TR[k], sd[0], expand(sd[0], sd[1]), classes(r["spec"])))
    for n in (128, 256):
        Lr = np.array([est("buildings", n, None, x[1]) for x in rows])
        Rr = np.array([est("transport", n, x[3], x[2]) for x in rows])
        res["estimate_error"][f"grid_{n}"] = {"left": stats(M.err(Lr, np.array([x[0]["left"] for x in rows], float))), "right": stats(M.err(Rr, np.array([x[0]["right"] for x in rows], float)))}
    names = ["ln_probes", "ln_right"]
    res["postgis_time_prediction"] = {}
    for label, n in (("current_planner_estimates", None), ("grid_128", 128), ("grid_256", 256)):
        D = with_grid("ms_disc", n) + with_grid("ms_cal", n)
        T = with_grid("ms_conf", n)
        w = M.fit(D, names)
        res["postgis_time_prediction"][label] = {"heldout_conf": stats(M.pred_ratio(T, names, w)), "in_sample": stats(M.pred_ratio(D, names, w))}
    # cost proxies (benchmark-time): per-query estimate latency of the numpy mirror, and metadata size of the grids
    w0 = rows[0][1]
    lat = {}
    for n in (128, 256):
        t0 = time.perf_counter()
        for _ in range(200):
            est("buildings", n, None, w0)
        lat[f"grid_{n}_us_per_buildings_estimate"] = round((time.perf_counter() - t0) / 200 * 1e6, 1)
    res["cost_proxies"] = {**lat, "grid_bytes_int32": {str(n): int(sum(G[k].nbytes for k in G.files if k.split("|")[1] == str(n) and k.split("|")[0] != "origin")) for n in (128, 256)},
                           "note": "numpy mirror of the planner estimator, not the Rust implementation; no Rust 256 grid exists, nothing is promoted"}
    (R / a.out).write_text(json.dumps(res, indent=1), encoding="utf-8")
    print(json.dumps(res, indent=1))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
