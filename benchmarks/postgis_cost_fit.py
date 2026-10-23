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

TERMS = ["startup", "scan", "result", "geom", "probe", "right"]


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
        (f["cardinality"] or 0) / 1000.0,
        (f["result_rows"] or 0) / 1000.0,
        ((f["result_geom_bytes"] or 0.0) / 1048576.0),
        (sj["expected_probes"] / 1000.0) if sj else 0.0,
        (sj["right_rows"] / 1000.0) if sj else 0.0,
    ]


def usable(r: dict, engine: str) -> bool:
    e = r[engine]
    return e["status"] == "ok" and e.get("model_ms") is not None


def fit_class(rows: list[dict], engine: str, active: list[str]) -> dict:
    cols = [TERMS.index(t) for t in active]
    A = np.array([[terms_of(r)[c] for c in cols] for r in rows], dtype=float)
    y = np.array([r[engine]["model_ms"] for r in rows], dtype=float)
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
        a = max(r[engine]["model_ms"], 1.0)
        out.append(max(p / a, a / p))
    return out


def pct(v: list[float], q: float) -> float:
    s = sorted(v)
    return s[min(len(s) - 1, int(math.ceil(q * len(s))) - 1)] if s else float("nan")


def load_compact(path: Path) -> list[dict]:
    return [compact(r) for r in load_raw(path)]
