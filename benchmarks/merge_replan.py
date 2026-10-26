"""Join the TIMINGS of the earlier measured sets with the plan-time FEATURES of the current planner (histograms + candidate-work features).

A timing does not depend on the planner (each engine's `execute` is timed directly), but the features it is explained by do: the earlier runs were planned
without spatial histograms (footer statistics only), which is not what ships. The harness's plan-only mode (GF_COST_PLAN_ONLY=1) re-plans the stored cases
with the current planner; this script puts the two together. The DuckDB-only stress repetitions of the 60 s boundary are folded into the matching case
(median of completed runs; a timeout when most runs timed out).

    python benchmarks/merge_replan.py --out raw/historical-r3.jsonl
"""

from __future__ import annotations

import json
import statistics
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from postgis_cost_lib import load_raw  # noqa: E402

RAW = Path(__file__).resolve().parent / "results" / "postgis-cost" / "raw"
SETS = {"cal": ("calibration-r2", "calibration-replan"), "hel": ("heldout-r2", "heldout-replan"), "con": ("confirm-r2", "confirm-replan"), "fre": ("fresh", "fresh-replan"), "fr2": ("fresh2", "fresh2-replan")}


def spec_key(spec: dict) -> str:
    s = {k: v for k, v in spec.items() if k not in ("id",)}
    return json.dumps(s, sort_keys=True)


def main() -> int:
    out = Path(sys.argv[sys.argv.index("--out") + 1])
    stress_by_spec: dict[str, list[dict]] = {}
    for r in load_raw(RAW / "stress.jsonl"):
        stress_by_spec.setdefault(spec_key(r["spec"]), []).append(r["duckdb"])
    n = 0
    with out.open("w", encoding="utf-8", newline="\n") as f:
        for tag, (timings, plans) in SETS.items():
            planned = {r["id"]: r["planner"] for r in load_raw(RAW / f"{plans}.jsonl")}
            for r in load_raw(RAW / f"{timings}.jsonl"):
                rec = {k: r[k] for k in ("template", "region", "spec", "duckdb", "postgis", "comparable", "identical") if k in r}
                rec["id"] = f"{tag}:{r['id']}"
                rec["planner"] = planned.get(r["id"])
                extra = stress_by_spec.get(spec_key(r["spec"]))
                if extra:
                    runs = [r["duckdb"]] + extra
                    ok = [x["model_ms"] for x in runs if x["status"] == "ok"]
                    to = sum(1 for x in runs if x["status"] == "timeout")
                    d = dict(r["duckdb"])
                    if to * 2 > len(runs) or not ok:
                        d["status"], d["model_ms"] = "timeout", None
                    else:
                        d["status"], d["model_ms"] = "ok", statistics.median(ok)
                    d["replicates"] = len(runs)
                    d["replicate_timeouts"] = to
                    rec["duckdb"] = d
                f.write(json.dumps(rec, separators=(",", ":")) + "\n")
                n += 1
    print(n, "records ->", out)
    return 0


if __name__ == "__main__":
    sys.exit(main())
