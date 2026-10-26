"""Raw JSONL (git-ignored, with EXPLAIN text) -> the compact per-case file that is committed (features, both engines' times, identity, scan/index evidence).

    python benchmarks/postgis_cost_compact.py benchmarks/results/postgis-cost/raw/calibration.jsonl benchmarks/results/postgis-cost/calibration-cases.json
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from postgis_cost_fit import workload_class  # noqa: E402
from postgis_cost_lib import compact, load_raw  # noqa: E402


def main() -> int:
    src, dst = Path(sys.argv[1]), Path(sys.argv[2])  # then optional flags
    rows = []
    metric_only = "--metric-only" in sys.argv  # keep only the metric semi joins (the evidence of the metric-semijoin safety phase)
    for rec in load_raw(src):
        sj = ((rec.get("planner") or {}).get("workload") or {}).get("semi_join")
        if metric_only and not (sj and sj.get("predicate") == "within_distance"):
            continue
        row = compact(rec)
        row["workload_class"] = workload_class(row)
        rows.append(row)
    dst.write_text(json.dumps({"source": src.name, "cases": rows}, separators=(",", ":")) + "\n", encoding="utf-8", newline="\n")
    print(dst, len(rows), "cases,", round(dst.stat().st_size / 1024), "KiB")
    return 0


if __name__ == "__main__":
    sys.exit(main())
