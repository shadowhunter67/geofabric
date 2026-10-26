"""Log-linear time models for the metric semi join, fitted with timeouts as CENSORED observations (Tobit), and their timeout-risk behaviour.

    ln(time_ms) ~ Normal(b0 + sum(b_i * x_i), sigma)      x_i = ln of a plan-time feature (the planner's own estimate, histogram-based)

A timed-out run contributes P(time > timeout) to the likelihood instead of a fake time, so the 11 DuckDB timeouts constrain the model like the ~700
completed runs do. The same model gives the expected time (cost) and the probability of exceeding the timeout (risk). A plan whose feature is unknown
or outside the measured envelope is answered "unknown" at planning time (Rust: geo_executor::cost_model::LogLinear).

    python benchmarks/metric_semijoin_fit.py select  <raw jsonl>... --engine duckdb|postgis     # leave-fold-out comparison of feature sets
    python benchmarks/metric_semijoin_fit.py fit     <raw jsonl>... --out models/metric-loglinear.json --features ln_probes ln_candidate_pairs ...
    python benchmarks/metric_semijoin_fit.py risk    --model models/metric-loglinear.json <raw jsonl>...      # timeout-risk quality on a set

Only records produced with the histogram-based planner carry the candidate-work features; older records are skipped (their `candidate_pairs` is null).
"""

from __future__ import annotations

import itertools
import json
import math
import sys
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))
from postgis_cost_fit import nelder_mead  # noqa: E402
from postgis_cost_lib import load_raw  # noqa: E402

TIMEOUT_MS = 60_000.0
LN_T = math.log(TIMEOUT_MS)
FEATURES = ["ln_probes", "ln_right_rows", "ln_right_rows_filtered", "ln_distance_m", "ln_candidate_pairs", "ln_candidates_per_probe", "ln_right_density", "ln_overlap_ratio", "ln_right_window_km2", "ln_left_window_km2"]
SRC = {"ln_probes": "expected_probes", "ln_right_rows": "right_rows", "ln_right_rows_filtered": "right_rows_filtered", "ln_distance_m": "distance_m", "ln_candidate_pairs": "candidate_pairs",
       "ln_candidates_per_probe": "candidates_per_probe", "ln_right_density": "right_density_per_km2", "ln_overlap_ratio": "overlap_ratio", "ln_right_window_km2": "right_window_km2",
       "ln_left_window_km2": "left_window_km2"}


def records(paths: list[Path]) -> list[dict]:
    out = []
    for p in paths:
        for r in load_raw(p):
            sj = ((r.get("planner") or {}).get("workload") or {}).get("semi_join")
            if not sj or sj.get("predicate") != "within_distance" or sj.get("candidate_pairs") is None:
                continue
            x = {}
            for f, k in SRC.items():
                v = sj.get(k)
                x[f] = math.log(max(v, 1e-9)) if v is not None else None
            row = {"id": f"{p.stem}:{r['id']}", "x": x, "sj": sj}
            for e in ("duckdb", "postgis"):
                st = r[e]["status"]
                row[e] = {"status": st, "ms": r[e]["model_ms"] if st == "ok" else None}
            out.append(row)
    return out


def phi(z: np.ndarray) -> np.ndarray:
    return 0.5 * (1.0 + np.vectorize(math.erf)(z / math.sqrt(2.0)))


def log_sf(z: float) -> float:
    """ln P(Z > z), stable in the tail."""
    if z < 6:
        return math.log(max(0.5 * math.erfc(z / math.sqrt(2.0)), 1e-300))
    return -0.5 * z * z - math.log(z * math.sqrt(2 * math.pi))


def observations(rows: list[dict], engine: str, feats: list[str]):
    X, y, cens = [], [], []
    for r in rows:
        e = r[engine]
        if e["status"] == "ok":
            yy, c = math.log(max(e["ms"], 1.0)), False
        elif e["status"] == "timeout":
            yy, c = LN_T, True
        else:
            continue
        if any(r["x"][f] is None for f in feats):
            continue
        X.append([1.0] + [r["x"][f] for f in feats])
        y.append(yy)
        cens.append(c)
    return np.array(X), np.array(y), np.array(cens)


def tobit(X: np.ndarray, y: np.ndarray, cens: np.ndarray) -> tuple[np.ndarray, float]:
    beta0 = np.linalg.lstsq(X, y, rcond=None)[0]
    s0 = float(np.std(y - X @ beta0)) or 1.0

    def nll(theta):
        beta, ls = theta[:-1], theta[-1]
        s = math.exp(ls)
        mu = X @ beta
        z = (y - mu) / s
        ll = np.where(cens, 0.0, -ls - 0.5 * z * z)
        for i in np.where(cens)[0]:
            ll[i] = log_sf((LN_T - mu[i]) / s)
        return -float(ll.sum()) + 1e-4 * float(np.sum(beta[1:] ** 2))  # a whisper of ridge keeps collinear features finite

    th = nelder_mead(nll, list(beta0) + [math.log(s0)], iters=1500)
    th = nelder_mead(nll, list(th), iters=700)
    return th[:-1], math.exp(th[-1])


def cv(rows: list[dict], engine: str, feats: list[str], k: int = 5, seed: int = 1) -> dict:
    X, y, cens = observations(rows, engine, feats)
    idx = np.random.RandomState(seed).permutation(len(y))
    err, pt, ev = [], [], []
    for f in range(k):
        te = idx[f::k]
        tr = np.setdiff1d(idx, te)
        beta, sigma = tobit(X[tr], y[tr], cens[tr])
        mu = X[te] @ beta
        done = ~cens[te]
        err += list(np.abs(mu[done] - y[te][done]))
        p = 1.0 - phi((LN_T - mu) / sigma)
        pt += list(p)
        ev += list(cens[te])
    r = np.exp(np.array(err))
    pt, ev = np.array(pt), np.array(ev)
    out = {"n": len(y), "timeouts": int(ev.sum()), "median_ratio": float(np.median(r)), "p90_ratio": float(np.percentile(r, 90))}
    if ev.any():
        out["p_at_timeouts_min"] = float(pt[ev].min())
        out["p_at_timeouts_median"] = float(np.median(pt[ev]))
        out["p_at_completed_p95"] = float(np.percentile(pt[~ev], 95))
        out["p_at_completed_max"] = float(pt[~ev].max())
    return out


def cmd_select(args: list[str]) -> int:
    engine = args[args.index("--engine") + 1]
    paths = [Path(a) for a in args if not a.startswith("--") and a not in (engine,)]
    rows = records(paths)
    print(f"{len(rows)} records with candidate-work features; engine {engine}")
    res = []
    core = ["ln_probes", "ln_right_rows_filtered", "ln_distance_m", "ln_candidate_pairs", "ln_right_density", "ln_overlap_ratio"]
    for k in (2, 3):
        for combo in itertools.combinations(core, k):
            try:
                c = cv(rows, engine, list(combo))
            except Exception as e:  # singular fits are simply skipped
                continue
            res.append((c["p90_ratio"], c["median_ratio"], combo, c))
    res.sort()
    for p90, med, combo, c in res[:10]:
        extra = f"  p@timeouts min {c.get('p_at_timeouts_min', 0):.2f} med {c.get('p_at_timeouts_median', 0):.2f} | p@completed p95 {c.get('p_at_completed_p95', 0):.2f} max {c.get('p_at_completed_max', 0):.2f}" if "p_at_timeouts_min" in c else ""
        print(f"  CV median x{med:.2f} P90 x{p90:.2f} (n={c['n']}, timeouts={c['timeouts']}) {combo}{extra}")
    return 0


def cmd_fit(args: list[str]) -> int:
    out = Path(args[args.index("--out") + 1])
    feats = args[args.index("--features") + 1:]
    paths = [Path(a) for a in args[: args.index("--out")] if not a.startswith("--")]
    engine = args[args.index("--engine") + 1] if "--engine" in args else None
    rows = records(paths)
    model = {}
    for e in ([engine] if engine else ["duckdb", "postgis"]):
        X, y, cens = observations(rows, e, feats)
        beta, sigma = tobit(X, y, cens)
        env = {f: [float(X[:, 1 + i].min()), float(X[:, 1 + i].max())] for i, f in enumerate(feats)}
        mu = X @ beta
        done = ~cens
        rat = np.exp(np.abs(mu[done] - y[done]))
        model[e] = {"features": feats, "intercept": float(beta[0]), "coefficients": [float(b) for b in beta[1:]], "sigma": float(sigma), "envelope": env, "extrapolation_ln": 1.0,
                    "n": int(len(y)), "timeouts": int(cens.sum()), "fit_ratio_median": float(np.median(rat)), "fit_ratio_p90": float(np.percentile(rat, 90))}
        print(e, {k: (round(v, 3) if isinstance(v, float) else v) for k, v in model[e].items() if k not in ("envelope", "features")}, model[e]["features"])
    out.write_text(json.dumps(model, indent=2) + "\n", encoding="utf-8", newline="\n")
    return 0


def predict_p(m: dict, x: dict) -> tuple[float | None, float | None, str]:
    """(P(timeout), expected ms, why-unknown) exactly as the Rust LogLinear::answer does."""
    mu = m["intercept"]
    for f, c in zip(m["features"], m["coefficients"]):
        v = x.get(f)
        if v is None:
            return None, None, f"{f} unknown"
        lo, hi = m["envelope"][f]
        if v < lo - m["extrapolation_ln"] or v > hi + m["extrapolation_ln"]:
            return None, None, f"{f} outside envelope"
        mu += c * v
    p = 1.0 - 0.5 * (1.0 + math.erf((LN_T - mu) / m["sigma"] / math.sqrt(2.0)))
    return p, math.exp(mu), ""


def cmd_risk(args: list[str]) -> int:
    model = json.loads(Path(args[args.index("--model") + 1]).read_text(encoding="utf-8"))
    paths = [Path(a) for a in args if not a.startswith("--") and not a.endswith("loglinear.json")]
    rows = records(paths)
    for e in ("duckdb", "postgis"):
        if e not in model:
            continue
        print("==", e)
        ev = [(r, predict_p(model[e], r["x"])) for r in rows if r[e]["status"] in ("ok", "timeout")]
        for th in (0.05, 0.10, 0.20, 0.30, 0.50):
            tp = sum(1 for r, (p, _, _) in ev if r[e]["status"] == "timeout" and p is not None and p >= th)
            fp = sum(1 for r, (p, _, _) in ev if r[e]["status"] == "ok" and p is not None and p >= th)
            T = sum(1 for r, _ in ev if r[e]["status"] == "timeout")
            print(f"  threshold {th:.2f}: flags {tp}/{T} timeouts, {fp} completed runs flagged (of {len(ev) - T})")
        print("  unknown:", sum(1 for _, (p, _, _) in ev if p is None))
    return 0


if __name__ == "__main__":
    cmd = sys.argv[1]
    rest = sys.argv[2:]
    sys.exit({"select": cmd_select, "fit": cmd_fit, "risk": cmd_risk}[cmd](rest))
