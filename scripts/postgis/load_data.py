#!/usr/bin/env python3
"""Load a GeoParquet file into the local PostGIS database and register it as a PostGIS-resident source.

  python scripts/postgis/load_data.py --parquet data/local/overture_places_hcm.parquet --table places \
         --crs OGC:CRS84 [--limit-rows N] [--logical-uri <uri as plans use it>]

For each file: create the table (owner gf_admin), COPY the rows, CREATE the GiST index on the geometry, ANALYZE, verify parity with
the Parquet file (row count and distinct ids), report geometry validity (reported, NEVER repaired: no silent mutation), and add the
entry to data/manifests/postgis-sources.json (what the gateway reads as GEOFABRIC_POSTGIS_SOURCES).

*** NOT RUN IN THE PHASE THAT WROTE IT *** (no database was installed: the user declined the download). Unverified; read before use.

Rules: geometries are stored exactly as the file holds them in SRID of --crs (4326 for OGC:CRS84: x = lon, y = lat; see
crates/geo-engine-postgis/src/crs.rs); columns of types the adapter does not lower (structs such as the GeoParquet `bbox` covering
column, lists...) are not stored and are reported; ODbL attribution of Overture-derived rows still applies to the copy
(docs/DATA-LICENSES.md).
"""
import argparse
import json
import os
import pathlib
import subprocess
import sys

import pyarrow as pa
import pyarrow.parquet as pq

ROOT = pathlib.Path(__file__).resolve().parent.parent.parent
BASE = ROOT / "tools" / ".cache" / "postgres"
PSQL = BASE / "pg16" / "pgsql" / "bin" / "psql.exe"
PORT = 5433
DB = "geofabric"
MANIFEST = ROOT / "data" / "manifests" / "postgis-sources.json"


def admin_password() -> str:
    return json.loads((BASE / "credentials.json").read_text())["gf_admin"]


def psql(args, sql=None, stdin=None):
    env = dict(os.environ, PGPASSWORD=admin_password())
    cmd = [str(PSQL), "-h", "127.0.0.1", "-p", str(PORT), "-U", "gf_admin", "-d", DB, "-v", "ON_ERROR_STOP=1", "-At", *args]
    if sql is not None:
        cmd += ["-c", sql]
    return subprocess.run(cmd, input=stdin, text=True, capture_output=True, env=env, check=True).stdout


def pg_type(t: pa.DataType):
    if pa.types.is_integer(t):
        return "bigint", "int"
    if pa.types.is_floating(t):
        return "double precision", "float"
    if pa.types.is_string(t) or pa.types.is_large_string(t):
        return "text", "text"
    if pa.types.is_boolean(t):
        return "boolean", "bool"
    return None, None


def ewkb_hex(wkb: bytes, srid: int) -> str:
    """Little-endian ISO/OGC WKB (2D) -> hex EWKB carrying the SRID, so COPY into geometry(Geometry, srid) accepts it."""
    if wkb[0] != 1:
        raise ValueError("big-endian WKB is not supported by this loader")
    t = int.from_bytes(wkb[1:5], "little")
    if t & 0x20000000 or t > 7:
        raise ValueError("only plain 2D WKB geometry types 1..7 are supported by this loader")
    return (wkb[:1] + (t | 0x20000000).to_bytes(4, "little") + srid.to_bytes(4, "little") + wkb[5:]).hex()


def csv_cell(v) -> str:
    if v is None:
        return ""
    s = str(v).replace('"', '""')
    return f'"{s}"'


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--parquet", required=True)
    ap.add_argument("--table", required=True)
    ap.add_argument("--crs", default="OGC:CRS84")
    ap.add_argument("--logical-uri")
    ap.add_argument("--limit-rows", type=int)
    ap.add_argument("--schema", default="geofabric")
    a = ap.parse_args()
    srid = 4326 if a.crs == "OGC:CRS84" else int(a.crs.split(":")[1])
    path = pathlib.Path(a.parquet).resolve()
    pf = pq.ParquetFile(path)
    geo = json.loads(pf.schema_arrow.metadata[b"geo"])
    gcol = geo["primary_column"]
    cols, dropped = {}, []
    for f in pf.schema_arrow:
        if f.name == gcol:
            continue
        pt, kind = pg_type(f.type)
        (cols.__setitem__(f.name, (pt, kind)) if pt else dropped.append(f.name))
    ddl = ", ".join(f'"{n}" {pt}' for n, (pt, _) in cols.items())
    t = f'"{a.schema}"."{a.table}"'
    psql([], f'DROP TABLE IF EXISTS {t}; CREATE TABLE {t} ({ddl}, "{gcol}" geometry(Geometry, {srid}))')
    order = list(cols) + [gcol]
    copy = f'\\copy {t} ({", ".join(chr(34) + c + chr(34) for c in order)}) FROM STDIN WITH (FORMAT csv)'
    loaded, distinct_ids = 0, set()
    for batch in pf.iter_batches(batch_size=50_000, columns=order):
        rows = batch.to_pylist()
        lines = []
        for r in rows:
            if a.limit_rows is not None and loaded >= a.limit_rows:
                break
            lines.append(",".join([csv_cell(r[c]) for c in cols] + [ewkb_hex(r[gcol], srid)]))
            loaded += 1
            if "id" in r:
                distinct_ids.add(r["id"])
        if lines:
            psql(["-c", copy], stdin="\n".join(lines) + "\n")
        if a.limit_rows is not None and loaded >= a.limit_rows:
            break
    psql([], f'CREATE INDEX ON {t} USING GIST ("{gcol}"); ANALYZE {t}')
    n_pg = int(psql([], f"SELECT count(*) FROM {t}").strip())
    expected = loaded if a.limit_rows is not None else pf.metadata.num_rows
    invalid = int(psql([], f'SELECT count(*) FROM {t} WHERE NOT ST_IsValid("{gcol}")').strip())
    srids = psql([], f'SELECT DISTINCT ST_SRID("{gcol}") FROM {t}').split()
    est = int(float(psql([], f"SELECT reltuples::bigint FROM pg_class WHERE oid = '{t}'::regclass").strip() or n_pg))
    ok = n_pg == expected and srids == [str(srid)]
    print(json.dumps({"table": a.table, "rows_parquet_loaded": expected, "rows_postgis": n_pg, "parity": ok, "srid_values": srids,
                      "invalid_geometries_reported_not_repaired": invalid, "columns_not_stored": dropped}, indent=1))
    if not ok:
        sys.exit("PARITY FAILED: not registering this source")
    entry = {"logical_uri": a.logical_uri or str(path), "schema": a.schema, "relation": a.table, "geometry_column": gcol, "crs": a.crs, "srid": srid,
             "columns": {n: k for n, (_, k) in cols.items()}, "row_estimate": est, "spatially_indexed": True}
    existing = json.loads(MANIFEST.read_text()) if MANIFEST.exists() else []
    existing = [e for e in existing if e["logical_uri"] != entry["logical_uri"]] + [entry]
    MANIFEST.parent.mkdir(parents=True, exist_ok=True)
    MANIFEST.write_text(json.dumps(existing, indent=1))
    print("registered in", MANIFEST)


if __name__ == "__main__":
    main()
