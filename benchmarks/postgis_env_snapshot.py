"""Benchmark environment metadata for the PostGIS cost-model evidence (benchmarks/results/postgis-cost/env-*.json).

Records what the brief requires next to every benchmark generation: machine, OS, CPU, RAM, PostgreSQL / PostGIS / DuckDB versions, GeoFabric commit,
dataset row counts (exact count(*), not reltuples), index inventory, ANALYZE status, load. No credential is read into the output: the admin role is
used through tools/postgis_portable.py (credentials.json stays where it is).

    python benchmarks/postgis_env_snapshot.py <out.json> [--model-version postgis-cost-v1]
"""

from __future__ import annotations

import json
import os
import platform
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
import postgis_portable as pg  # noqa: E402


def sh(*cmd: str) -> str:
    return subprocess.run(list(cmd), cwd=ROOT, capture_output=True, text=True, encoding="utf-8").stdout.strip()


def psql_rows(sql: str) -> list[list[str]]:
    creds = json.loads((pg.BASE / "credentials.json").read_text())
    out = pg.psql(sql, db=pg.DB, password=creds["gf_admin"]).stdout.strip()
    return [line.split("|") for line in out.splitlines() if line]


def cpu_and_ram() -> tuple[str, float]:
    ps = ["powershell", "-NoProfile", "-Command",
          "(Get-CimInstance Win32_Processor | Select-Object -First 1 -ExpandProperty Name); [math]::Round((Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory/1GB,1)"]
    lines = sh(*ps).splitlines()
    return (lines[0].strip() if lines else "unknown"), (float(lines[1]) if len(lines) > 1 else 0.0)


def main() -> int:
    out = Path(sys.argv[1])
    model = sys.argv[sys.argv.index("--model-version") + 1] if "--model-version" in sys.argv else "postgis-cost-v1"
    cpu, ram = cpu_and_ram()
    tables = [r[0] for r in psql_rows("SELECT relname FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='geofabric' AND c.relkind='r' ORDER BY 1")]
    counts = {t: int(psql_rows(f"SELECT count(*) FROM geofabric.{t}")[0][0]) for t in tables}
    analyzed = {r[0]: r[1] or None for r in psql_rows("SELECT relname, coalesce(greatest(last_analyze,last_autoanalyze)::text,'') FROM pg_stat_user_tables WHERE schemaname='geofabric' ORDER BY 1")}
    indexes = [{"table": r[0], "index": r[1], "def": r[2]} for r in psql_rows("SELECT tablename, indexname, indexdef FROM pg_indexes WHERE schemaname='geofabric' ORDER BY 1,2")]
    settings = {r[0]: r[1] for r in psql_rows("SELECT name, setting FROM pg_settings WHERE name IN ('shared_buffers','work_mem','effective_cache_size','max_parallel_workers_per_gather','random_page_cost','jit')")}
    manifests = {}
    for m in sorted((ROOT / "data" / "manifests").glob("overture_*.json")):
        d = json.loads(m.read_text(encoding="utf-8"))
        manifests[m.stem] = {k: d.get(k) for k in ("release", "version", "retrieved_at", "sha256") if k in d}
    duck = ROOT / "tools" / ".cache" / "duckdb-1.5.6"
    snapshot = {
        "captured_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "machine": platform.node(),
        "os": f"{platform.system()} {platform.release()} ({platform.version()})",
        "cpu": cpu,
        "logical_cpus": os.cpu_count(),
        "ram_gb": ram,
        "postgresql": psql_rows("SELECT version()")[0][0],
        "postgis": psql_rows("SELECT postgis_full_version()")[0][0],
        "duckdb": duck.name if duck.exists() else "unknown",
        "geofabric_commit": sh("git", "rev-parse", "HEAD"),
        "geofabric_dirty": bool(sh("git", "status", "--porcelain")),
        "dataset_versions": manifests,
        "dataset_row_counts": counts,
        "analyze_last": analyzed,
        "indexes": indexes,
        "postgres_settings": settings,
        "model_version": model,
        "cache_terminology": "process-cold = PostgreSQL restarted just before; the OS file cache is NOT controlled (never called 'cold cache'). warm-repeat = repetitions after the first run.",
    }
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(snapshot, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({k: snapshot[k] for k in ("machine", "cpu", "ram_gb", "postgresql", "dataset_row_counts")}, indent=1)[:1500])
    return 0


if __name__ == "__main__":
    sys.exit(main())
