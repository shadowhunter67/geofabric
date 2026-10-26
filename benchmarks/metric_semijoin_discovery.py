"""Feature discovery for the metric semi join (plan-time features only).

For every measured metric semi join (within_distance) it derives, from the request window, the distance and the planner's own estimates, features
the planner can know BEFORE execution, then checks how well each explains each engine's time (Spearman rank correlation, in log space) and the
engine ORDER. Nothing here reads a result row count, a runtime counter or a timing as a feature.

    python benchmarks/metric_semijoin_discovery.py <raw jsonl>... [--json out.json]
"""

from __future__ import annotations

import json
import math
import sys
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))
from postgis_cost_lib import load_raw  # noqa: E402

TIMEOUT_MS = 60_000.0
KM_PER_DEG_LAT = 110.574
KM_PER_DEG_LON_EQ = 111.320


def area_km2(b: list[float]) -> float:
    lat = (b[1] + b[3]) / 2
    return max((b[2] - b[0]) * KM_PER_DEG_LON_EQ * math.cos(math.radians(lat)) * (b[3] - b[1]) * KM_PER_DEG_LAT, 1e-6)


def expand(b: list[float], d_m: float) -> list[float]:
    lat = max(abs(b[1]), abs(b[3]))
    dlat = d_m / 110_574.0 * 1.02
    dlon = d_m / (111_320.0 * max(math.cos(math.radians(lat)), 0.01)) * 1.02
    return [b[0] - dlon, b[1] - dlat, b[2] + dlon, b[3] + dlat]


def spec_bbox_dist(spec: dict) -> tuple[list[float], float] | None:
    r = spec.get("request")
    if r and r.get("kind") in ("near_count", "near"):
        return r["bbox"], float(r["with"]["distance_m"])
    if spec.get("name") == "near_ids":
        return spec["params"]["bbox"], float(spec["params"]["distance_m"])
    return None


def cases(paths: list[Path]) -> list[dict]:
    out = []
    for p in paths:
        for rec in load_raw(p):
            wl = (rec.get("planner") or {}).get("workload") or {}
            sj = wl.get("semi_join")
            sd = spec_bbox_dist(rec["spec"])
            if not sj or sj.get("predicate") != "within_distance" or not sd:
                continue
            bbox, d = sd
            ex = expand(bbox, d)
            a_left, a_right = area_km2(bbox), area_km2(ex)
            probes, right = sj["expected_probes"], sj["right_rows"]
            dens = right / a_right  # right features per km^2 inside the grown window
            cand_per_probe = dens * math.pi * (d / 1000.0) ** 2
            f = {
                "id": rec["id"], "key": f"{p.stem}:{rec['id']}", "src": p.name, "distance_m": d, "probes": probes, "right_rows": right, "left_area_km2": a_left, "right_area_km2": a_right,
                "left_density": probes / a_left, "right_density": dens, "cand_per_probe": cand_per_probe, "pairs": probes * cand_per_probe,
                "overlap_ratio": a_left / a_right, "out_rows": wl.get("result_rows"),
                "filtered_right": bool(rec["spec"].get("request", {}).get("with", {}).get("filter") or rec["spec"].get("params", {}).get("road_classes")),
            }
            for e in ("duckdb", "postgis"):
                x = rec[e]
                f[e + "_status"] = x["status"]
                f[e + "_ms"] = x["model_ms"] if x["status"] == "ok" else (TIMEOUT_MS if x["status"] == "timeout" else None)
            out.append(f)
    return out


def spearman(x: np.ndarray, y: np.ndarray) -> float:
    rx = np.argsort(np.argsort(x)).astype(float)
    ry = np.argsort(np.argsort(y)).astype(float)
    return float(np.corrcoef(rx, ry)[0, 1])


FEATURES = ["probes", "right_rows", "distance_m", "right_density", "left_density", "cand_per_probe", "pairs", "overlap_ratio", "right_area_km2", "out_rows"]


def main() -> int:
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    cs = cases([Path(a) for a in args])
    print(len(cs), "metric semi-join cases")
    for e in ("duckdb", "postgis"):
        rows = [c for c in cs if c[e + "_ms"] is not None]
        y = np.log(np.maximum([c[e + "_ms"] for c in rows], 1.0))
        print(f"\n== {e}: n={len(rows)} (timeouts {sum(1 for c in rows if c[e + '_status'] == 'timeout')})")
        for f in FEATURES:
            v = np.array([c[f] if c[f] is not None else np.nan for c in rows], float)
            ok = ~np.isnan(v)
            print(f"  {f:16s} spearman(log time) = {spearman(np.log1p(v[ok]), y[ok]):+.2f}")
    both = [c for c in cs if c["duckdb_ms"] is not None and c["postgis_ms"] is not None]
    print(f"\n== ENGINE ORDER (log ratio duckdb/postgis), n={len(both)}")
    y = np.log(np.maximum([c["duckdb_ms"] for c in both], 1.0) / np.maximum([c["postgis_ms"] for c in both], 1.0))
    for f in FEATURES:
        v = np.array([c[f] if c[f] is not None else np.nan for c in both], float)
        ok = ~np.isnan(v)
        print(f"  {f:16s} spearman(log ratio) = {spearman(np.log1p(v[ok]), y[ok]):+.2f}")
    if "--json" in sys.argv:
        Path(sys.argv[sys.argv.index("--json") + 1]).write_text(json.dumps(cs, separators=(",", ":")), encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
