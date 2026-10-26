"""Fixtures that pin the PYTHON fit/evaluation of the metric models to the RUST planner code (crates/geo-executor/tests/cost_model_parity.rs).

For a sample of measured metric semi joins it records the raw plan-time features the planner produced, and what the SHIPPED model files (the ones the
engines embed) answer for them according to an independent Python implementation: expected time of each engine, the timeout probability, the risk level,
and the workload class. Rust must reproduce all of it.

    python benchmarks/make_metric_fixtures.py
"""

from __future__ import annotations

import json
import math
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(Path(__file__).resolve().parent))
from metric_semijoin_fit import SRC, records  # noqa: E402

RAW = ROOT / "benchmarks" / "results" / "postgis-cost" / "raw"
OUT = ROOT / "benchmarks" / "results" / "postgis-cost" / "metric-fixtures.json"


def ll_answer(m: dict, x: dict, timeout_ms: float):
    mu = m["intercept"]
    for f, c in zip(m["features"], m["coefficients"]):
        v = x.get(f)
        if v is None:
            return ("unknown", f"{f} unknown")
        lo, hi = m["envelope"][f]
        if v < lo - m["extrapolation_ln"] or v > hi + m["extrapolation_ln"]:
            return ("unknown", f"{f} outside")
        mu += c * v
    p = 1.0 - 0.5 * (1.0 + math.erf((math.log(timeout_ms) - mu) / m["sigma"] / math.sqrt(2.0)))
    return ("known", math.exp(mu), p)


def workload_class(sj: dict) -> str:
    if sj["predicate"] == "intersects":
        return "semijoin_intersects"
    if sj["expected_probes"] <= 5000.0:
        return "semijoin_metric_small"
    pairs = sj.get("candidate_pairs")
    if sj["expected_probes"] <= 50000.0 and pairs is not None and pairs < 1.0e7:
        return "semijoin_metric_medium"
    return "semijoin_metric_large"


def verdict(spec: dict, x: dict) -> dict:
    r = x.get(spec["regime_feature"])
    if r is None:
        return {"level": "unknown", "p": None}
    if r < spec["regime_min_ln"]:
        return {"level": "low", "p": None}
    a = ll_answer(spec["model"], x, spec["timeout_ms"])
    if a[0] == "unknown":
        return {"level": "unknown", "p": None}
    p = a[2]
    return {"level": "timeout_risk" if p >= spec["timeout_risk_at"] else "elevated" if p >= spec["elevated_at"] else "low", "p": p}


def main() -> int:
    models = {e: json.loads((ROOT / f"crates/geo-engine-{e}/src/cost_model_v1.json").read_text(encoding="utf-8")) for e in ("postgis", "duckdb")}
    rows = records([RAW / "msj_cal.jsonl"])
    # the sample deliberately includes all four DuckDB timeouts, the biggest plans, and the smallest
    rows.sort(key=lambda r: r["sj"]["candidate_pairs"])
    pick = rows[:6] + rows[len(rows) // 2 - 6: len(rows) // 2 + 6] + rows[-18:]
    out = []
    for r in pick:
        x = r["x"]
        sj = r["sj"]
        cls = workload_class(sj)
        item = {"features": {k: sj.get(k) for k in ("expected_probes", "right_rows", "right_rows_filtered", "distance_m", "candidate_pairs", "candidates_per_probe", "right_density_per_km2", "overlap_ratio", "right_window_km2", "left_window_km2", "predicate", "metric")}, "class": cls}
        for e in ("postgis", "duckdb"):
            ll = models[e]["classes"][cls]["loglinear"]
            a = ll_answer(ll, x, 60000.0)
            item[e] = {"time_ms": a[1] if a[0] == "known" else None, "risk": verdict(models[e]["timeout_risk"], x)}
        out.append(item)
    OUT.write_text(json.dumps({"models": {e: models[e]["model_version"] for e in models}, "rows": out}, indent=1), encoding="utf-8", newline="\n")
    print(len(out), "fixtures ->", OUT)
    return 0


if __name__ == "__main__":
    sys.exit(main())
