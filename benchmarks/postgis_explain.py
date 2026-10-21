#!/usr/bin/env python3
"""EXPLAIN / EXPLAIN ANALYZE of the statements GeoFabric lowers for PostGIS (development evidence only, never product execution).

  python benchmarks/postgis_explain.py --dump results/postgis/sql-slice.json [--case NAME ...] [--analyze] [--timeout-ms 120000] [--out file.json]

The dump comes from the live cross-check harness (GF_XE_DUMP). Each statement is PREPAREd with its bound parameters and run through
EXPLAIN (BUFFERS, FORMAT TEXT [, ANALYZE]); the output reports which scan nodes the plan uses (Index Scan / Bitmap Index Scan / Seq Scan),
estimated and actual rows, planning and execution time. It uses the SELECT-only account like the product does.
"""
import argparse
import json
import os
import re
import subprocess
import sys

BASE = os.path.join(os.environ["LOCALAPPDATA"], "geofabric-runtime", "postgres")
PSQL = os.path.join(BASE, "pg16", "pgsql", "bin", "psql.exe")
CRED = json.load(open(os.path.join(BASE, "credentials.json")))


def lit(p):
    t, v = p["t"], p["v"]
    if t == "text":
        return "'" + v.replace("'", "''") + "'"
    if t == "boolean":
        return "true" if v else "false"
    return repr(v)


def explain(item, analyze, timeout_ms, user="gf_reader"):
    args = ", ".join(lit(p) for p in item["params"])
    opts = "ANALYZE, BUFFERS" if analyze else "BUFFERS"
    sql = f"SET statement_timeout = {timeout_ms}; PREPARE p AS {item['sql']}; EXPLAIN ({opts}) EXECUTE p({args});"
    r = subprocess.run([PSQL, "-h", "127.0.0.1", "-p", "5433", "-U", user, "-d", "geofabric", "-At", "-v", "ON_ERROR_STOP=1", "-c", sql],
                       capture_output=True, env=dict(os.environ, PGPASSWORD=CRED[user], PGCLIENTENCODING="UTF8"))
    out = r.stdout.decode("utf-8", "replace").replace("\r\n", "\n")
    if r.returncode != 0:
        return {"error": r.stderr.decode("utf-8", "replace")[-600:]}
    nodes = re.findall(r"(Index Only Scan|Index Scan|Bitmap Index Scan|Bitmap Heap Scan|Seq Scan|Nested Loop|Hash Join|Hash Semi Join|Nested Loop Semi Join|Aggregate|Sort|Limit)(?: using (\S+))?", out)
    scans = [{"node": n, "index": i or None} for n, i in nodes]
    plan_ms = re.search(r"Planning Time: ([\d.]+) ms", out)
    exec_ms = re.search(r"Execution Time: ([\d.]+) ms", out)
    return {"plan": out, "nodes": scans, "planning_ms": float(plan_ms.group(1)) if plan_ms else None, "execution_ms": float(exec_ms.group(1)) if exec_ms else None}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--dump", required=True)
    ap.add_argument("--case", nargs="*")
    ap.add_argument("--analyze", action="store_true")
    ap.add_argument("--timeout-ms", type=int, default=120000)
    ap.add_argument("--out")
    ap.add_argument("--show", action="store_true", help="print the full plan text")
    a = ap.parse_args()
    items = json.load(open(a.dump))
    res = {}
    for it in items:
        if a.case and it["case"] not in a.case:
            continue
        if "error" in it:
            res[it["case"]] = {"error": it["error"]}
            continue
        r = explain(it, a.analyze, a.timeout_ms)
        res[it["case"]] = r
        used = sorted({f"{n['node']}{' on ' + n['index'] if n['index'] else ''}" for n in r.get("nodes", []) if "Scan" in n["node"] or "Join" in n["node"] or "Loop" in n["node"]})
        print(f"{it['case']:<42} plan={r.get('planning_ms')} exec={r.get('execution_ms')} ms | {'; '.join(used) if used else r.get('error', '')[:200]}")
        if a.show:
            print(r.get("plan", ""))
    if a.out:
        json.dump(res, open(a.out, "w"), indent=1)


if __name__ == "__main__":
    main()
