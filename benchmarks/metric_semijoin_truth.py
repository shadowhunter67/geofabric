"""True left / right row counts of each metric semi join (BENCHMARK-TIME ONLY: the planner never sees these).

Used to separate three kinds of error that were mixed in the earlier model: (1) cardinality-estimation error (planner's estimate vs the real count),
(2) candidate-density error, (3) engine cost-model error. The counts use the same semantics as the plan: a `BboxFilter` is "the covering envelope
overlaps the window", the right side is grown by the distance (in degrees, as geo-plan does) and carries the road-class filter.

    python benchmarks/metric_semijoin_truth.py <raw jsonl>... --out truth.json     (PostgreSQL must be running)
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
sys.path.insert(0, str(Path(__file__).resolve().parent))
import postgis_portable as pg  # noqa: E402
from metric_semijoin_discovery import expand, spec_bbox_dist  # noqa: E402
from postgis_cost_lib import load_raw  # noqa: E402


def overlap(b: list[float]) -> str:
    return f"bbox_xmin <= {b[2]} AND bbox_xmax >= {b[0]} AND bbox_ymin <= {b[3]} AND bbox_ymax >= {b[1]}"


def classes(spec: dict) -> list[str] | None:
    r = spec.get("request")
    if r:
        f = r["with"].get("filter")
        return f["values"] if f else None
    return spec["params"].get("road_classes")


def main() -> int:
    argv = sys.argv[1:]
    i = argv.index("--out")
    out = Path(argv[i + 1])
    args = argv[:i] + argv[i + 2:]
    sqls, ids = [], []
    for p in args:
        for rec in load_raw(Path(p)):
            sd = spec_bbox_dist(rec["spec"])
            if not sd:
                continue
            bbox, d = sd
            ex = expand(bbox, d)
            cl = classes(rec["spec"])
            cond = f" AND class IN ({', '.join(repr(c) for c in cl)})" if cl else ""
            key = f"{Path(p).stem}:{rec['id']}"  # ids repeat across sets (fre-0000 exists in two): the file name is part of the key
            sqls.append(f"SELECT '{key}', (SELECT count(*) FROM geofabric.buildings WHERE {overlap(bbox)}), (SELECT count(*) FROM geofabric.transport WHERE {overlap(ex)}{cond}), (SELECT count(*) FROM geofabric.transport WHERE {overlap(ex)})")
            ids.append(key)
    creds = json.loads((pg.BASE / "credentials.json").read_text())
    res = []
    for k in range(0, len(sqls), 25):  # a Windows command line is limited to ~32 k characters: query in batches
        res += pg.psql(" UNION ALL ".join(sqls[k:k + 25]), db=pg.DB, password=creds["gf_admin"]).stdout.strip().splitlines()
    truth = {}
    for line in res:
        i, left, right, right_unfiltered = line.split("|")
        truth[i] = {"left": int(left), "right": int(right), "right_unfiltered": int(right_unfiltered)}
    out.write_text(json.dumps(truth, separators=(",", ":")), encoding="utf-8")
    print(len(truth), "cases ->", out)
    return 0


if __name__ == "__main__":
    sys.exit(main())
