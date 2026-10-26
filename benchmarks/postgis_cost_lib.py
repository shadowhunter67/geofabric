"""Shared helpers for the PostGIS cost-model evidence: raw JSONL -> compact per-case rows, EXPLAIN text -> index/scan facts.

The raw files (benchmarks/results/postgis-cost/raw/*.jsonl) hold EXPLAIN text and every run; they are git-ignored. `compact()` keeps what a model
or a reviewer needs (features, both engines' times, identity, scan/index evidence) and is what gets committed.
"""

from __future__ import annotations

import json
import re
from pathlib import Path

NODE_RE = re.compile(r"^\s*(?:->\s+)?(?P<node>[A-Z][A-Za-z ]+?)(?:\s+using\s+(?P<idx>\S+))?(?:\s+on\s+(?P<rel>\S+))?\s+\(cost=")
BUF_RE = re.compile(r"Buffers: (?P<b>.*)")
TIME_RE = re.compile(r"(Planning|Execution) Time: ([0-9.]+) ms")
ACTUAL_RE = re.compile(r"actual time=[0-9.]+\.\.([0-9.]+) rows=(\d+) loops=(\d+)")


def explain_facts(lines: list[str] | dict | None) -> dict:
    """Plan node types, index names, actual rows/loops of scans and joins, shared hit/read blocks of the top node, planning/execution time."""
    if not lines or isinstance(lines, dict):
        return {"available": False}
    nodes: list[dict] = []
    top_buffers: dict[str, int] = {}
    times: dict[str, float] = {}
    for ln in lines:
        m = TIME_RE.search(ln)
        if m:
            times[m.group(1).lower() + "_ms"] = float(m.group(2))
            continue
        n = NODE_RE.match(ln)
        if n:
            a = ACTUAL_RE.search(ln)
            nodes.append({
                "node": n.group("node").strip(), "index": n.group("idx"), "relation": n.group("rel"),
                "rows": int(a.group(2)) if a else None, "loops": int(a.group(3)) if a else None,
            })
            continue
        b = BUF_RE.match(ln.strip())
        if b and not top_buffers:
            for k, v in re.findall(r"(shared hit|shared read)=(\d+)", b.group("b")):
                top_buffers[k.replace("shared ", "")] = int(v)
    types = [n["node"] for n in nodes]
    return {
        "available": True,
        "node_types": types,
        "indexes": sorted({n["index"] for n in nodes if n["index"]}),
        "seq_scan": any(t.startswith("Seq Scan") or t.startswith("Parallel Seq Scan") for t in types),
        "index_scan": any("Index" in t for t in types),
        "bitmap_scan": any("Bitmap" in t for t in types),
        "nested_loop": any(t.startswith("Nested Loop") for t in types),
        "hash_join": any("Hash" in t for t in types),
        "parallel": any(t.startswith("Parallel") or t == "Gather" for t in types),
        "scans": [n for n in nodes if "Scan" in n["node"]][:6],
        "shared_hit": top_buffers.get("hit"),
        "shared_read": top_buffers.get("read"),
        **times,
    }


def load_raw(path: Path) -> list[dict]:
    return [json.loads(l) for l in path.read_text(encoding="utf-8").splitlines() if l.strip()]


def compact(rec: dict) -> dict:
    """One committed row per case."""
    wl = (rec.get("planner") or {}).get("workload") or {}
    sj = wl.get("semi_join")
    row = {
        "id": rec["id"], "template": rec["template"], "region": rec.get("region"), "spec": rec["spec"].get("request") or {"name": rec["spec"].get("name"), **rec["spec"].get("params", {})},
        "engine_order": rec.get("engine_order"),
        "features": {k: wl.get(k) for k in ("bbox_selectivity", "cardinality", "result_rows", "result_rows_high", "shape", "has_bbox_filter", "has_filter", "has_sort", "has_aggregate", "has_intersects", "has_transform", "limit", "limit_certain", "bounded_by_limit", "estimate_quality", "result_geom_bytes", "result_attr_bytes")},
        "sources": [{"selected_rows": s["selected_rows"], "total_rows": s["total_rows"], "name": Path(s["uri"].replace("\\", "/")).name} for s in wl.get("sources", [])],
        "semi_join": ({k: sj[k] for k in ("left_rows", "right_rows", "metric", "predicate", "distance_m", "expected_probes", "candidate_pairs", "candidates_per_probe", "right_rows_filtered", "right_density_per_km2", "overlap_ratio", "estimate_basis")} if sj else None),
        "duckdb": {k: rec["duckdb"].get(k) for k in ("status", "first_ms", "model_ms", "rows", "digest")},
        "postgis": {k: rec["postgis"].get(k) for k in ("status", "first_ms", "model_ms", "rows", "digest")},
        "comparable": rec.get("comparable"), "identical": rec.get("identical"),
        "planner_chosen": (rec.get("planner") or {}).get("chosen"),
        "planner_scores": {c["engine"]: c["score_ms"] for c in (rec.get("planner") or {}).get("candidates", []) if c.get("feasible")},
        "explain": explain_facts(rec.get("explain")),
    }
    return row
