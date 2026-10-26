"""Put the metric semi-join models (pooled log-linear cost model + regime timeout-risk model) into the engines' shipped model files.

Only the three metric classes change. Every other class keeps the coefficients (and the evidence) of the v2 fit - the validated classes are NOT refitted in
this phase. The validation file is rewritten for the new model version: classes whose model did not change keep their level and say why; the metric classes
start UNVALIDATED until the confirmation set says otherwise.

    python benchmarks/assemble_metric_models.py --cost models/metric-loglinear.json \
        --pg-risk models/pg-risk.json --dd-risk models/dd-risk.json --elevated 0.2 --timeout-risk 0.5 \
        --pg-version postgis-cost-v3 --dd-version duckdb-semijoin-cost-v3 --commit <sha> --seeds 20261201 20261205
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
METRIC = ("semijoin_metric_small", "semijoin_metric_medium", "semijoin_metric_large")
FILES = {
    "postgis": (ROOT / "crates/geo-engine-postgis/src/cost_model_v1.json", ROOT / "crates/geo-engine-postgis/src/cost_validation_v1.json"),
    "duckdb": (ROOT / "crates/geo-engine-duckdb/src/cost_model_v1.json", ROOT / "crates/geo-engine-duckdb/src/cost_validation_v1.json"),
}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--cost", type=Path, required=True)
    ap.add_argument("--pg-risk", type=Path, required=True)
    ap.add_argument("--dd-risk", type=Path, required=True)
    ap.add_argument("--elevated", type=float, required=True)
    ap.add_argument("--timeout-risk", type=float, required=True)
    ap.add_argument("--pg-version", required=True)
    ap.add_argument("--dd-version", required=True)
    ap.add_argument("--commit", required=True)
    ap.add_argument("--seeds", type=int, nargs="+", required=True)
    ap.add_argument("--cases", type=int, required=True, help="number of metric semi-join cases the models were fitted on")
    a = ap.parse_args()
    cost = json.loads(a.cost.read_text(encoding="utf-8"))
    for engine, (mpath, vpath) in FILES.items():
        model = json.loads(mpath.read_text(encoding="utf-8"))
        old_version = model["model_version"]
        ll = cost[engine]
        risk = json.loads((a.pg_risk if engine == "postgis" else a.dd_risk).read_text(encoding="utf-8"))
        proto = model["classes"].get("semijoin_metric_small") or model["classes"]["semijoin_metric_large"]
        for c in METRIC:
            entry = dict(model["classes"].get(c) or proto)
            entry["loglinear"] = {k: ll[k] for k in ("features", "intercept", "coefficients", "sigma", "envelope", "extrapolation_ln")}
            entry["n_calibration"] = ll["n"]
            entry["fit_ratio_median"] = round(ll["fit_ratio_median"], 3)
            entry["fit_ratio_p90"] = round(ll["fit_ratio_p90"], 3)
            model["classes"][c] = entry
        model["timeout_risk"] = {"version": f"{engine}-timeout-risk-v1", "timeout_ms": 60000.0, "elevated_at": a.elevated, "timeout_risk_at": a.timeout_risk,
                                 "regime_feature": risk["regime_feature"], "regime_min_ln": risk["regime_min_ln"], "model": risk["model"]}
        model["model_version"] = a.pg_version if engine == "postgis" else a.dd_version
        prov = model["provenance"]
        prov["benchmark_commit"] = a.commit
        prov["calibration_seeds"] = sorted(set(prov.get("calibration_seeds", [])) | set(a.seeds))
        prov["method"] = (prov["method"] + f" | {model['model_version']}: the three metric semi-join classes use a pooled log-linear Tobit model (timeouts censored) on the histogram-based plan-time "
                          f"features of {a.cases} measured metric semi joins (seeds {a.seeds} + earlier sets re-planned); their timeout risk comes from a regime-restricted Tobit model; "
                          f"all other classes keep the {old_version} coefficients")
        mpath.write_text(json.dumps(model, indent=2) + "\n", encoding="utf-8", newline="\n")
        val = json.loads(vpath.read_text(encoding="utf-8"))
        val["model_version"] = model["model_version"]
        for c in METRIC:
            val["classes"].pop(c, None)  # unvalidated until the confirmation set
        for c, v in val["classes"].items():
            if not v["reason"].startswith("unchanged"):
                v["reason"] = f"unchanged since {old_version} (validation evidence of that version): " + v["reason"]
        vpath.write_text(json.dumps(val, indent=2) + "\n", encoding="utf-8", newline="\n")
        print(engine, model["model_version"], "classes:", sorted(model["classes"]), "validated:", [c for c, v in val["classes"].items() if v["level"] == "validated"])
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
