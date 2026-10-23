"""Fit the interpretable per-class cost models from the CALIBRATION set only, and evaluate them on any measured set.

Model form (shared by both engines, one set of non-negative coefficients per workload class; every term has a meaning and a unit):

    time_ms = startup                                   fixed cost of one statement (connection checkout/plan/dispatch)
            + scan_us_per_row   * cardinality / 1000     rows that match the window and are read/filtered
            + result_us_per_row * result_rows / 1000     rows materialised and returned
            + geom_ms_per_mb    * result_geom_mb         geometry bytes serialised
            + probe_us          * expected_probes / 1000 semi join: right-side probes (one per left row)
            + right_us_per_row  * right_rows / 1000      semi join: right-side rows prepared (index build / projection)

Coefficients are found by non-negative least squares on the RELATIVE error (rows weighted by 1/time), so the 17 ms and the 17 s cases both count.
Classes are NOT fitted by dataset name: they come from `workload_class()` below, which mirrors the Rust rule (geo-engine-postgis/src/cost.rs).

    python benchmarks/postgis_cost_fit.py fit  --cal raw/calibration.jsonl --engine postgis --out models/postgis-cost-v1.json
    python benchmarks/postgis_cost_fit.py eval --model ... --set raw/heldout.jsonl
"""

from __future__ import annotations

import json
import math
import sys
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))
from postgis_cost_lib import compact, load_raw  # noqa: E402

TERMS = ["startup", "scan", "result", "result_geometry", "probe", "right"]
CARDINALITY_BROAD = 300_000  # mirror of geo_executor::cost_model (a Rust test pins both)
PROBES_SMALL = 5_000.0
TIMEOUT_MS = 60_000.0  # a timed-out run is a censored observation: at least this long


def scan_rows(r: dict) -> float:
    """Mirror of geo_executor::cost_model::scan_rows: an unordered non-aggregate LIMIT stops reading at `limit` rows."""
    f = r["features"]
    n = f["limit"]
    if n is not None and not f["has_sort"] and not f["has_aggregate"] and not r["semi_join"]:
        return min(float(f["cardinality"] or 0), float(n))
    return float(f["cardinality"] or 0)


def workload_class(r: dict) -> str:
    """Mirror of geo_executor::cost_model::workload_class over a compact case row."""
    f, sj = r["features"], r["semi_join"]
    if sj:
        if sj["predicate"] == "intersects":
            return "semijoin_intersects"
        return "semijoin_metric_small" if sj["expected_probes"] <= PROBES_SMALL else "semijoin_metric_large"
    if scan_rows(r) > CARDINALITY_BROAD:
        return "broad_scan"
    if f["has_aggregate"]:
        return "aggregate"
    if f["has_intersects"] or f["has_transform"]:
        return "indexed_spatial"
    return "indexed_bbox"


def nnls(A: np.ndarray, b: np.ndarray, iters: int = 5000) -> np.ndarray:
    """Lawson-Hanson style active-set NNLS (small problems; no scipy)."""
    n = A.shape[1]
    x = np.zeros(n)
    passive: list[int] = []
    w = A.T @ (b - A @ x)
    for _ in range(iters):
        active = [j for j in range(n) if j not in passive]
        if not active or w[active].max() <= 1e-10:
            break
        passive.append(active[int(np.argmax(w[active]))])
        while True:
            s = np.zeros(n)
            sol, *_ = np.linalg.lstsq(A[:, passive], b, rcond=None)
            s[passive] = sol
            if (s[passive] > 0).all():
                x = s
                break
            neg = [j for j in passive if s[j] <= 0]
            alpha = min(x[j] / (x[j] - s[j]) for j in neg if x[j] - s[j] != 0) if neg else 1.0
            x = x + alpha * (s - x)
            passive = [j for j in passive if x[j] > 1e-12]
            if not passive:
                break
        w = A.T @ (b - A @ x)
    return x


def terms_of(r: dict) -> list[float]:
    f = r["features"]
    sj = r["semi_join"]
    return [
        1.0,
        scan_rows(r) / 1000.0,
        (f["result_rows"] or 0) / 1000.0,
        ((f["result_geom_bytes"] or 0.0) / 1048576.0),
        (sj["expected_probes"] / 1000.0) if sj else 0.0,
        (sj["right_rows"] / 1000.0) if sj else 0.0,
    ]


def usable(r: dict, engine: str) -> bool:
    e = r[engine]
    return e["status"] == "ok" and e.get("model_ms") is not None


def actual_ms(r: dict, engine: str) -> float | None:
    """Measured time, or the censoring bound for a timeout (at least TIMEOUT_MS); None when the engine produced nothing usable."""
    e = r[engine]
    if e["status"] == "ok" and e.get("model_ms") is not None:
        return e["model_ms"]
    return TIMEOUT_MS if e["status"] == "timeout" else None


def fit_class(rows: list[dict], engine: str, active: list[str]) -> dict:
    cols = [TERMS.index(t) for t in active]
    A = np.array([[terms_of(r)[c] for c in cols] for r in rows], dtype=float)
    y = np.array([actual_ms(r, engine) for r in rows], dtype=float)
    w = 1.0 / np.maximum(y, 5.0)  # relative error weighting with a 5 ms floor (timer noise)
    coef = nnls(A * w[:, None], y * w)
    return {"terms": dict(zip(active, [float(c) for c in coef])), "n": len(rows)}


def predict(model: dict, r: dict) -> float:
    t = terms_of(r)
    return sum(model["terms"].get(name, 0.0) * t[i] for i, name in enumerate(TERMS))


def ratios(rows: list[dict], model: dict, engine: str) -> list[float]:
    out = []
    for r in rows:
        p = max(predict(model, r), 1.0)
        a = max(actual_ms(r, engine) or 1.0, 1.0)
        out.append(max(p / a, a / p))
    return out


def pct(v: list[float], q: float) -> float:
    s = sorted(v)
    return s[min(len(s) - 1, int(math.ceil(q * len(s))) - 1)] if s else float("nan")


def load_compact(path: Path) -> list[dict]:
    return [compact(r) for r in load_raw(path)]


# ---------------------------------------------------------------------------------------------------------------------------------- CLI

ROW_TERMS = ["startup", "scan", "result", "result_geometry"]
SEMI_TERMS = ["startup", "scan", "result", "probe", "right"]
# DuckDB keeps its measured shared formula everywhere except the semi-join shapes it was never calibrated on (26x off on the admin join)
ENGINE_CLASSES = {
    "postgis": ["indexed_bbox", "indexed_spatial", "aggregate", "broad_scan", "semijoin_intersects", "semijoin_metric_small", "semijoin_metric_large"],
    "duckdb": ["semijoin_intersects", "semijoin_metric_small", "semijoin_metric_large"],
}
MODEL_VERSION = {"postgis": "postgis-cost-v1", "duckdb": "duckdb-semijoin-cost-v1"}
SUFFIX = ""  # set by --version-suffix (e.g. ".1" -> postgis-cost-v1.1)


def build_model(cal_rows: list[dict], engine: str, meta: dict) -> dict:
    classes: dict = {}
    for c in ENGINE_CLASSES[engine]:
        rows = [r for r in cal_rows if workload_class(r) == c and actual_ms(r, engine) is not None]
        if len(rows) < 6:
            continue
        m = fit_class(rows, engine, SEMI_TERMS if c.startswith("semijoin") else ROW_TERMS)
        rat = ratios(rows, m, engine)
        classes[c] = {
            "terms": {k: round(v, 6) for k, v in m["terms"].items()},
            "n_calibration": m["n"],
            "fit_ratio_median": round(sorted(rat)[len(rat) // 2], 3),
            "fit_ratio_p90": round(pct(rat, 0.9), 3),
        }
    return {"model_version": MODEL_VERSION[engine], "engine": engine, "provenance": meta, "classes": classes}


def main(argv: list[str]) -> int:
    import argparse

    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    f = sub.add_parser("fit")
    f.add_argument("--cal", type=Path, required=True, nargs="+", help="one or more measured sets; all of them are CALIBRATION data for this model version")
    f.add_argument("--version-suffix", default="")
    f.add_argument("--seeds", type=int, nargs="+", default=[20261103])
    f.add_argument("--out-dir", type=Path, required=True, help="directory receiving postgis-cost-v1.json and duckdb-semijoin-cost-v1.json")
    f.add_argument("--commit", required=True)
    f.add_argument("--env", type=Path, required=True)
    a = ap.parse_args(argv)
    rows = [r for c in a.cal for r in load_compact(c)]
    env = json.loads(a.env.read_text(encoding="utf-8"))
    meta = {
        "benchmark_commit": a.commit,
        "calibration_seed": a.seeds[0],
        "calibration_seeds": a.seeds,
        "calibration_cases": len(rows),
        "dataset_versions": f"Overture {list(env['dataset_versions'].values())[0].get('release', '2026-09-23.1') if env.get('dataset_versions') else '2026-09-23.1'}; PostGIS rows " + ", ".join(f"{k}={v}" for k, v in env["dataset_row_counts"].items() if not k.startswith("fx_")),
        "machine_class": f"{env['cpu']}, {env['ram_gb']} GB RAM, {env['os'].split(' (')[0]}, loopback PostgreSQL + DuckDB on the same host ({env['postgresql'].split(',')[0]}; PostGIS 3.6.2)",
        "build_profile": "release (cargo --release); engine `execute` wall time, planning excluded",
        "fitted_at": __import__("time").strftime("%Y-%m-%d", __import__("time").gmtime()),
        "method": "non-negative least squares per workload class on relative error (weight 1/max(time,5 ms)); DuckDB timeouts enter as censored observations at 60000 ms",
    }
    a.out_dir.mkdir(parents=True, exist_ok=True)
    for eng in ("postgis", "duckdb"):
        model = build_model(rows, eng, meta)
        model["model_version"] = MODEL_VERSION[eng] + a.version_suffix
        out = a.out_dir / f"{model['model_version']}.json"
        out.write_text(json.dumps(model, indent=2) + "\n", encoding="utf-8", newline="\n")
        print(out, {k: (v["n_calibration"], v["fit_ratio_median"], v["fit_ratio_p90"]) for k, v in model["classes"].items()})
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
