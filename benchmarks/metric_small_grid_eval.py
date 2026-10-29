"""Left/right estimate error of centre-count grids at several resolutions vs PostgreSQL truth (benchmark-time)."""
import json, sys, time
from pathlib import Path
import numpy as np
sys.path.insert(0, str(Path(__file__).resolve().parent)); sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
from metric_semijoin_discovery import expand, spec_bbox_dist
from metric_semijoin_truth import classes
from postgis_cost_lib import load_raw
from metric_small_models import R, TR, err, q
G = np.load(R / "raw" / "grids.npz")
def est(table, n, cls, w):
    o = G[f"origin|{n}"]; ix0, iy0, cx, cy = o
    if table == "buildings": a = G[f"buildings|{n}|all"].astype(float)
    else:
        ks = [k for k in G.files if k.startswith(f"transport|{n}|")]
        a = sum(G[k].astype(float) for k in ks if (not cls or k.split("|")[2] in cls))
    x0 = 104.999 + ix0 * cx; y0 = 8.607 + iy0 * cy
    fx = np.clip(np.minimum((np.arange(a.shape[1]) + 1) * cx + x0, w[2]) - np.maximum(np.arange(a.shape[1]) * cx + x0, w[0]), 0, None) / cx
    fy = np.clip(np.minimum((np.arange(a.shape[0]) + 1) * cy + y0, w[3]) - np.maximum(np.arange(a.shape[0]) * cy + y0, w[1]), 0, None) / cy
    return float(fy @ a @ fx)
rows = []
for split in ("ms_disc", "ms_cal"):
    for r in load_raw(R / "raw" / f"{split}.jsonl"):
        k = f"{split}:{r['id']}"; sd = spec_bbox_dist(r["spec"])
        if k in TR and sd: rows.append((TR[k], sd[0], expand(sd[0], sd[1]), classes(r["spec"]), r["planner"]["workload"]["semi_join"]))
print(len(rows), "cases")
print("current planner (128 grid): left", q(err(np.array([x[4]['expected_probes'] for x in rows]), np.array([x[0]['left'] for x in rows], float))), "| right", q(err(np.array([x[4]['right_rows_filtered'] for x in rows]), np.array([x[0]['right'] for x in rows], float))))
for n in (128, 256, 512, 1024, 2048):
    L = np.array([est("buildings", n, None, x[1]) for x in rows]); Rr = np.array([est("transport", n, x[3], x[2]) for x in rows])
    print(f"grid {n:5d}: left {q(err(L, np.array([x[0]['left'] for x in rows], float)))} | right {q(err(Rr, np.array([x[0]['right'] for x in rows], float)))} | cells {n*n} (~{n*n*4/1e6:.1f} MB int32 full extent)")

# time-model effect: replace planner left/right by grid estimates, fit on disc, evaluate on cal
import metric_small_models as M
def withgrid(split, n):
    out = []
    byid = {r["id"]: r for r in M.load(split)}
    for r in load_raw(R / "raw" / f"{split}.jsonl"):
        if r["id"] not in byid: continue
        sd = spec_bbox_dist(r["spec"]); 
        if not sd: continue
        d = dict(byid[r["id"]]); d["probes"] = max(est("buildings", n, None, sd[0]), 1); d["right"] = max(est("transport", n, classes(r["spec"]), expand(sd[0], sd[1])), 1); out.append(d)
    return out
print("\nPostGIS time model [ln_probes, ln_right] with grid-derived estimates (fit disc -> held-out cal)")
for n in (128, 256, 512, 1024):
    D, C = withgrid("ms_disc", n), withgrid("ms_cal", n); w = M.fit(D, ["ln_probes", "ln_right"])
    print(f"  grid {n:5d}: in-sample {M.q(M.pred_ratio(D,['ln_probes','ln_right'],w))} | held-out {M.q(M.pred_ratio(C,['ln_probes','ln_right'],w))}")
