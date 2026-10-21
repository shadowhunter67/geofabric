#!/usr/bin/env python3
"""Load a GeoParquet file (or a spatial clip of it) into the local PostGIS database, verify parity, register it as a resident source.

  python scripts/postgis/load_data.py --parquet <file> --table <name> --crs OGC:CRS84 [--clip-bbox x0 y0 x1 y1] [--logical-uri <uri>]

Steps for each file: create the table (owner gf_admin), COPY the rows (EWKB carrying the SRID), CREATE the GiST index on the geometry,
ANALYZE, then PARITY against the Parquet source (the same clip applied to both sides):
  row count; the ID SET (equal, not just the count); null-geometry count; geometry-type histogram; SRID values; and an ORDER-INDEPENDENT
  digest of (id, WKB bytes) computed independently in Python and in PostgreSQL. Geometry validity is REPORTED, never repaired.
Any mismatch stops the script without registering the source. On success the entry (with `coverage` when clipped) is merged into
data/manifests/postgis-sources.json, which the gateway reads as GEOFABRIC_POSTGIS_SOURCES.

A clipped copy records its coverage in the catalog and the engine refuses to answer outside it (crates/geo-engine-postgis/src/lower.rs).
Geometries are stored exactly as the file holds them; columns of types the adapter does not lower (the GeoParquet `bbox` struct, lists...)
are not stored and are reported. Overture-derived rows keep their ODbL obligations in the copy (docs/DATA-LICENSES.md).
"""
import argparse
import collections
import struct
import hashlib
import json
import os
import pathlib
import subprocess
import sys
import time

import pyarrow as pa
import pyarrow.dataset as ds
import pyarrow.parquet as pq

ROOT = pathlib.Path(__file__).resolve().parent.parent.parent
BASE = pathlib.Path(os.environ.get("GEOFABRIC_RUNTIME_DIR") or (pathlib.Path(os.environ["LOCALAPPDATA"]) / "geofabric-runtime" / "postgres"))  # same as tools/postgis_portable.py
PSQL = BASE / "pg16" / "pgsql" / "bin" / "psql.exe"
PORT = 5433
DB = "geofabric"
MANIFEST = ROOT / "data" / "manifests" / "postgis-sources.json"
TYPE_NAMES = {1: "POINT", 2: "LINESTRING", 3: "POLYGON", 4: "MULTIPOINT", 5: "MULTILINESTRING", 6: "MULTIPOLYGON", 7: "GEOMETRYCOLLECTION"}


def admin_password() -> str:
    return json.loads((BASE / "credentials.json").read_text())["gf_admin"]


def psql(args, sql=None, stdin=None):
    # UTF-8 end to end: the data holds Vietnamese names, and the Windows locale codec (cp1252) would fail or mangle them
    env = dict(os.environ, PGPASSWORD=admin_password(), PGCLIENTENCODING="UTF8")
    cmd = [str(PSQL), "-h", "127.0.0.1", "-p", str(PORT), "-U", "gf_admin", "-d", DB, "-v", "ON_ERROR_STOP=1", "-At", *args]
    if sql is not None:
        cmd += ["-c", sql]
    r = subprocess.run(cmd, input=stdin.encode("utf-8") if stdin is not None else None, capture_output=True, env=env)
    if r.returncode != 0:
        sys.exit("psql failed: " + r.stderr.decode("utf-8", "replace")[-1500:])
    return r.stdout.decode("utf-8").replace(chr(13) + chr(10), chr(10))  # psql on Windows ends lines with CRLF


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
    """Little-endian OGC WKB (2D) -> hex EWKB carrying the SRID, so COPY into geometry(Geometry, srid) accepts it."""
    if wkb[0] != 1:
        raise ValueError("big-endian WKB is not supported by this loader")
    t = int.from_bytes(wkb[1:5], "little")
    if t & 0x20000000 or t > 7:
        raise ValueError("only plain 2D WKB geometry types 1..7 are supported by this loader")
    return (wkb[:1] + (t | 0x20000000).to_bytes(4, "little") + srid.to_bytes(4, "little") + wkb[5:]).hex()


def csv_cell(v) -> str:
    if v is None:
        return ""
    return '"' + str(v).replace('"', '""') + '"'


def row_digest(rid, wkb: bytes) -> int:
    """60-bit order-independent contribution of one row: md5(id ':' hex(WKB)); the same formula runs in PostgreSQL."""
    return int(hashlib.md5(f"{rid}:{wkb.hex()}".encode()).hexdigest()[:15], 16)


def cov_row_digest(rid, box) -> int:
    """60-bit contribution of the covering box: md5(id ':' big-endian IEEE-754 bytes of the four doubles), bit-exact (PG: float8send)."""
    hexed = "".join(struct.pack(">d", v).hex() if v is not None else "null" for v in box)
    return int(hashlib.md5(f"{rid}:{hexed}".encode()).hexdigest()[:15], 16)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--parquet", required=True)
    ap.add_argument("--table", required=True)
    ap.add_argument("--crs", default="OGC:CRS84")
    ap.add_argument("--logical-uri")
    ap.add_argument("--clip-bbox", type=float, nargs=4, metavar=("X0", "Y0", "X1", "Y1"), help="load only rows whose bbox intersects this window (CRS of the file)")
    ap.add_argument("--schema", default="geofabric")
    ap.add_argument("--metric-srid", type=int, action="append", default=[], help="also index ST_Transform(geometry, SRID) (e.g. 32648 for Ho Chi Minh City): without it the metric semi-join is an all-pairs nested loop")
    ap.add_argument("--evidence", help="write the parity/index evidence JSON here")
    a = ap.parse_args()
    t_start = time.time()
    srid = 4326 if a.crs == "OGC:CRS84" else int(a.crs.split(":")[1])
    path = pathlib.Path(a.parquet).resolve()
    pf = pq.ParquetFile(path)
    geo = json.loads(pf.schema_arrow.metadata[b"geo"])
    gcol = geo["primary_column"]
    cols, dropped = {}, []
    covering = None
    for f in pf.schema_arrow:
        if f.name == gcol:
            continue
        if f.name == "bbox" and pa.types.is_struct(f.type) and {"xmin", "ymin", "xmax", "ymax"} <= {f.type.field(i).name for i in range(f.type.num_fields)}:
            # the GeoParquet covering box: stored as four double columns. Engines that read the file filter on THIS box (it is padded
            # relative to the geometry, so `geometry && window` would give another answer), so the database must keep it bit for bit.
            covering = {k: f"bbox_{k}" for k in ("xmin", "ymin", "xmax", "ymax")}
            for k in covering.values():
                cols[k] = ("double precision", "float")
            continue
        pt, kind = pg_type(f.type)
        (cols.__setitem__(f.name, (pt, kind)) if pt else dropped.append(f.name))
    if "id" not in cols:
        sys.exit("the loader needs an `id` column (parity is checked on the ID set)")
    flt = None
    if a.clip_bbox:
        x0, y0, x1, y1 = a.clip_bbox
        flt = (ds.field("bbox", "xmin") <= x1) & (ds.field("bbox", "xmax") >= x0) & (ds.field("bbox", "ymin") <= y1) & (ds.field("bbox", "ymax") >= y0)
    ddl = ", ".join(f'"{n}" {pt}' for n, (pt, _) in cols.items())
    t = f'"{a.schema}"."{a.table}"'
    psql([], f'DROP TABLE IF EXISTS {t}; CREATE TABLE {t} ({ddl}, "{gcol}" geometry(Geometry, {srid}))')
    order = [c for c in cols if not (covering and c in covering.values())] + ([("bbox")] if covering else []) + [gcol]
    csv_cols = list(cols) + [gcol]  # COPY order
    copy = f'\\copy {t} ({", ".join(chr(34) + c + chr(34) for c in csv_cols)}) FROM STDIN WITH (FORMAT csv)'
    read_cols = [c for c in order]
    scanner = ds.dataset(str(path), format="parquet").scanner(columns=read_cols, filter=flt, batch_size=50_000)
    n_src, ids, digest, null_geoms, types, cov_digest = 0, [], 0, 0, collections.Counter(), 0
    for batch in scanner.to_batches():
        lines = []
        cols_py = {c: batch.column(c).to_pylist() for c in order if c != "bbox"}
        if covering:
            sa = batch.column("bbox")
            for k, name in covering.items():
                cols_py[name] = sa.field(k).to_pylist()
        for i in range(batch.num_rows):
            g = cols_py[gcol][i]
            rid = cols_py["id"][i]
            ids.append(rid)
            n_src += 1
            if g is None:
                null_geoms += 1
                lines.append(",".join([csv_cell(cols_py[c][i]) for c in cols] + [""]))
                continue
            types[TYPE_NAMES.get(int.from_bytes(g[1:5], "little") & 0xFF, "OTHER")] += 1
            digest += row_digest(rid, g)
            if covering:
                cov_digest += cov_row_digest(rid, [cols_py[covering[k]][i] for k in ("xmin", "ymin", "xmax", "ymax")])
            lines.append(",".join([csv_cell(cols_py[c][i]) for c in cols] + [ewkb_hex(g, srid)]))
        if lines:
            psql(["-c", copy], stdin="\n".join(lines) + "\n")
    t_loaded = time.time()
    idx = f"{a.table}_{gcol}_gix"
    psql([], f'CREATE INDEX "{idx}" ON {t} USING GIST ("{gcol}")')
    if covering:
        c = covering
        # functional GiST on the covering envelope: the lowered BboxFilter writes exactly this expression, so the planner can use it
        psql([], f'CREATE INDEX "{a.table}_covering_gix" ON {t} USING GIST (ST_MakeEnvelope("{c["xmin"]}", "{c["ymin"]}", "{c["xmax"]}", "{c["ymax"]}", {srid}))')
    metric_indexes = []
    for m in a.metric_srid:
        # the expression is EXACTLY what the PostGIS lowering writes for a Transform, so the planner can use the index for ST_DWithin on
        # the transformed geometry; results are unchanged (physical design only)
        psql([], f'CREATE INDEX "{a.table}_utm{m}_gix" ON {t} USING GIST (ST_Transform(ST_SetSRID("{gcol}", {srid}), {m}))')
        metric_indexes.append(m)
    psql([], f"ANALYZE {t}")
    t_indexed = time.time()

    # ---- parity -----------------------------------------------------------------------------------------------------------
    n_pg = int(psql([], f"SELECT count(*) FROM {t}").strip())
    null_pg = int(psql([], f'SELECT count(*) FROM {t} WHERE "{gcol}" IS NULL').strip())
    ids_pg = psql([], f'SELECT "id"::text FROM {t} ORDER BY "id"::text COLLATE "C"').split("\n")
    ids_pg = [x for x in ids_pg if x != ""]
    ids_src = sorted(str(x) for x in ids)
    dig_pg = int(psql([], f"SELECT coalesce(sum((('x'||substr(md5(\"id\"::text||':'||encode(ST_AsBinary(\"{gcol}\",'NDR'),'hex')),1,15))::bit(60)::bigint)::numeric),0) FROM {t} WHERE \"{gcol}\" IS NOT NULL").strip().split(".")[0])
    types_pg = {k: int(v) for k, v in (l.split("|") for l in psql([], f'SELECT GeometryType("{gcol}"), count(*) FROM {t} WHERE "{gcol}" IS NOT NULL GROUP BY 1').split("\n") if l)}
    srids = psql([], f'SELECT DISTINCT ST_SRID("{gcol}") FROM {t} WHERE "{gcol}" IS NOT NULL').split()
    invalid = int(psql([], f'SELECT count(*) FROM {t} WHERE "{gcol}" IS NOT NULL AND NOT ST_IsValid("{gcol}")').strip())
    cov_ok = True
    cov_pg = None
    if covering:
        c = covering
        expr = "||".join(f"encode(float8send(\"{c[k]}\"),'hex')" for k in ("xmin", "ymin", "xmax", "ymax"))
        cov_pg = int(psql([], f"SELECT coalesce(sum((('x'||substr(md5(\"id\"::text||':'||{expr}),1,15))::bit(60)::bigint)::numeric),0) FROM {t} WHERE \"{gcol}\" IS NOT NULL").strip().split(".")[0])
        cov_ok = cov_pg == cov_digest
    parity = {
        "rows": [n_src, n_pg], "id_set_equal": ids_src == ids_pg, "distinct_ids": [len(set(ids_src)), len(set(ids_pg))],
        "null_geometries": [null_geoms, null_pg], "geometry_types": [dict(types), types_pg], "srid_values": srids,
        "id_geometry_digest_equal": digest == dig_pg, "digest_source": digest, "digest_postgis": dig_pg,
        "covering_columns": covering, "covering_digest_equal": cov_ok if covering else None, "covering_digest_source": cov_digest if covering else None, "covering_digest_postgis": cov_pg,
    }
    ok = (n_src == n_pg and parity["id_set_equal"] and null_geoms == null_pg and dict(types) == types_pg
          and srids in ([str(srid)], []) and parity["id_geometry_digest_equal"] and cov_ok)
    # ---- index / ANALYZE evidence -----------------------------------------------------------------------------------------
    indexes = [l.split("|") for l in psql([], f"SELECT indexname, indexdef FROM pg_indexes WHERE schemaname='{a.schema}' AND tablename='{a.table}'").split("\n") if l]
    stats = psql([], f"SELECT reltuples::bigint, relpages, pg_total_relation_size('{t}'::regclass), pg_relation_size('{t}'::regclass) FROM pg_class WHERE oid='{t}'::regclass").strip().split("|")
    analyzed = psql([], f"SELECT coalesce(last_analyze::text, last_autoanalyze::text, 'never') FROM pg_stat_user_tables WHERE relid='{t}'::regclass").strip()
    ev = {"table": a.table, "source_file": str(path), "clip_bbox": a.clip_bbox, "columns_stored": list(cols), "columns_not_stored": dropped,
          "parity": parity, "parity_ok": ok, "invalid_geometries_reported_not_repaired": invalid,
          "indexes": [{"name": n, "definition": d} for n, d in indexes], "analyze": {"last_analyze": analyzed, "reltuples": int(stats[0]), "relpages": int(stats[1])},
          "sizes_bytes": {"table_with_indexes": int(stats[2]), "table_heap": int(stats[3])},
          "seconds": {"copy": round(t_loaded - t_start, 1), "index_and_analyze": round(t_indexed - t_loaded, 1)}}
    print(json.dumps(ev, indent=1))
    if a.evidence:
        pathlib.Path(a.evidence).write_text(json.dumps(ev, indent=1))
    if not ok:
        sys.exit("PARITY FAILED: not registering this source")
    entry = {"logical_uri": a.logical_uri or str(path), "schema": a.schema, "relation": a.table, "geometry_column": gcol, "crs": a.crs, "srid": srid,
             "columns": {n: k for n, (_, k) in cols.items() if not (covering and n in covering.values())}, "row_estimate": n_pg, "spatially_indexed": True,
             "coverage": a.clip_bbox, "metric_indexes": metric_indexes, "covering": ({"xmin": covering["xmin"], "ymin": covering["ymin"], "xmax": covering["xmax"], "ymax": covering["ymax"]} if covering else None)}
    existing = json.loads(MANIFEST.read_text()) if MANIFEST.exists() else []
    existing = [e for e in existing if e["logical_uri"] != entry["logical_uri"]] + [entry]
    MANIFEST.parent.mkdir(parents=True, exist_ok=True)
    MANIFEST.write_text(json.dumps(existing, indent=1))
    print("registered in", MANIFEST)


if __name__ == "__main__":
    main()
