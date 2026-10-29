"""Ground truth + finer spatial grids for the metric_small research (BENCHMARK-TIME ONLY: the planner never sees any of this).

For every metric semi join of the given raw sets it records, from PostgreSQL:
  left          buildings whose bbox overlaps the window (the probe count the plan should have estimated)
  right         roads (with the road-class filter) whose bbox overlaps the window grown by the distance (the right side the plan should have estimated)
  right_all     the same without the class filter
  pairs         CANDIDATE PAIRS: (building, road) with the road's bbox overlapping the building's bbox grown by the distance, road filter applied.
                This is the index-candidate work a metric semi join does before the exact distance test, i.e. what `candidate_pairs` tries to predict.
and, once, the centre-count grids of buildings and roads at several resolutions over the HCMC coverage (what a finer histogram would hold).

    python benchmarks/metric_small_truth.py truth <raw jsonl>... --out truth.json
    python benchmarks/metric_small_truth.py grids --out grids.npz
"""

from __future__ import annotations

import json
import sys
import time
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
sys.path.insert(0, str(Path(__file__).resolve().parent))
import postgis_portable as pg  # noqa: E402
from metric_semijoin_discovery import expand, spec_bbox_dist  # noqa: E402
from metric_semijoin_truth import classes, overlap  # noqa: E402
from postgis_cost_lib import load_raw  # noqa: E402

COVERAGE = (106.35, 10.35, 107.05, 11.25)  # the PostGIS relation is clipped to this
HIST_BBOX = (104.999, 8.607, 108.001, 12.001)  # the planner's buildings histogram grid (128 x 128 over this)
RESOLUTIONS = [128, 256, 512, 1024, 2048]


def psql(sql: str) -> list[str]:
    creds = json.loads((pg.BASE / "credentials.json").read_text())
    return pg.psql(sql, db=pg.DB, password=creds["gf_admin"]).stdout.strip().splitlines()


def env(b: list[float]) -> str:
    return f"st_makeenvelope({b[0]},{b[1]},{b[2]},{b[3]},4326)"


def truth(paths: list[Path], out: Path) -> None:
    res, sqls, keys = {}, [], []
    for p in paths:
        for rec in load_raw(p):
            sd = spec_bbox_dist(rec["spec"])
            if not sd:
                continue
            bbox, d = sd
            ex = expand(bbox, d)
            cl = classes(rec["spec"])
            cond = f" AND t.class IN ({', '.join(repr(c) for c in cl)})" if cl else ""
            dlon, dlat = (ex[2] - ex[0] - (bbox[2] - bbox[0])) / 2, (ex[3] - ex[1] - (bbox[3] - bbox[1])) / 2
            key = f"{p.stem}:{rec['id']}"
            keys.append(key)
            sqls.append(
                f"SELECT '{key}', "
                f"(SELECT count(*) FROM geofabric.buildings WHERE {overlap(bbox)}), "
                f"(SELECT count(*) FROM geofabric.transport t WHERE {overlap(ex).replace('bbox_', 't.bbox_')}{cond}), "
                f"(SELECT count(*) FROM geofabric.transport t WHERE {overlap(ex).replace('bbox_', 't.bbox_')}), "
                f"(SELECT count(*) FROM (SELECT st_makeenvelope(bbox_xmin,bbox_ymin,bbox_xmax,bbox_ymax,4326) e FROM geofabric.buildings WHERE {overlap(bbox)}) b "
                f"JOIN geofabric.transport t ON st_makeenvelope(t.bbox_xmin,t.bbox_ymin,t.bbox_xmax,t.bbox_ymax,4326) && st_expand(b.e, {dlon:.8f}, {dlat:.8f}){cond})")
    t0 = time.time()
    for k in range(0, len(sqls), 10):
        for line in psql(" UNION ALL ".join(sqls[k:k + 10])):
            i, left, right, right_all, pairs = line.split("|")
            res[i] = {"left": int(left), "right": int(right), "right_all": int(right_all), "pairs": int(pairs)}
        if (k // 10) % 5 == 0:
            print(f"  {len(res)}/{len(sqls)} ({time.time() - t0:.0f} s)", flush=True)
    out.write_text(json.dumps(res, separators=(",", ":")), encoding="utf-8")
    print(len(res), "cases ->", out)


def grids(out: Path) -> None:
    """Centre-count grids over the SAME extent as the planner's histogram, at several resolutions (cell edge = span / n). Roads are also split by class."""
    x0, y0, x1, y1 = HIST_BBOX
    arrays = {}
    for n in RESOLUTIONS:
        cx, cy = (x1 - x0) / n, (y1 - y0) / n
        ix0, ix1 = int((COVERAGE[0] - x0) // cx), int((COVERAGE[2] - x0) // cx) + 1
        iy0, iy1 = int((COVERAGE[1] - y0) // cy), int((COVERAGE[3] - y0) // cy) + 1
        for table, by_class in (("buildings", False), ("transport", True)):
            t0 = time.time()
            cell = f"floor((((bbox_xmin+bbox_xmax)/2) - {x0}) / {cx})::int, floor((((bbox_ymin+bbox_ymax)/2) - {y0}) / {cy})::int"
            extra = ", class" if by_class else ""
            rows = psql(f"SELECT {cell}{extra}, count(*) FROM geofabric.{table} GROUP BY 1, 2{extra.replace(', class', ', 3') if by_class else ''}")
            classes_: dict[str, np.ndarray] = {}
            shape = (iy1 - iy0, ix1 - ix0)
            for line in rows:
                parts = line.split("|")
                ix, iy, cnt = int(parts[0]), int(parts[1]), int(parts[-1])
                cls = parts[2] if by_class else "all"
                a = classes_.setdefault(cls, np.zeros(shape, dtype=np.int32))
                if ix0 <= ix < ix1 and iy0 <= iy < iy1:
                    a[iy - iy0, ix - ix0] += cnt
            for cls, a in classes_.items():
                arrays[f"{table}|{n}|{cls}"] = a
            arrays[f"origin|{n}"] = np.array([ix0, iy0, cx, cy], dtype=np.float64)
            print(f"  {table} {n}x{n}: {len(rows)} cell rows, {time.time() - t0:.1f} s", flush=True)
    np.savez_compressed(out, **arrays)
    print("grids ->", out, round(out.stat().st_size / 1e6, 1), "MB")


def main() -> int:
    a = sys.argv[1:]
    if a and a[0] == "truth":
        i = a.index("--out")
        truth([Path(p) for p in a[1:i]], Path(a[i + 1]))
    elif a and a[0] == "grids":
        grids(Path(a[a.index("--out") + 1]))
    else:
        print(__doc__)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
