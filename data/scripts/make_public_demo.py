"""Build the compact PUBLIC DEMO pack (central Ho Chi Minh City, the Saigon River) for the static browser deployment.

It cuts one small window out of the normalized GeoParquet files that `fetch_overture.py` produced (same flat schema, same Overture release),
rewrites them with small row groups so the browser can prune by range request, and writes a manifest with sizes and SHA-256.

The result is a derived Overture database: it stays under the ODbL (share-alike) and must keep the attribution in `docs/DATA-LICENSES.md`
(shown on the page and in the map corner). It is NOT committed to Git history as a data dump of the full pack; only this small subset is
published, with the manifest.

    benchmarks/.venv/Scripts/python data/scripts/make_public_demo.py --out web/public/demo
"""

from __future__ import annotations

import argparse
import hashlib
import json
from datetime import datetime, timezone
from pathlib import Path

import duckdb
import pyarrow.parquet as pq

# Saigon River, District 1 and Thu Thiem: buildings, a street grid, POIs and wards within ~8 km.
DEFAULT_BBOX = [106.660, 10.740, 106.740, 10.810]
RELEASE = "2026-09-23.1"

# source id (the id the app and the data manifests use) -> (file relative to the repo, row group size, public file name, projection)
BUILDINGS_FLAT = (
    "id, names.primary AS name, subtype, class, height, num_floors, facade_material, roof_material, roof_shape, has_parts, version, geometry, bbox"
)
SOURCES = {
    "overture_buildings_vn_south": ("benchmarks/data/overture_buildings_vn_south.parquet", 4_000, "demo_buildings.parquet", BUILDINGS_FLAT),
    "overture_transport_hcm": ("data/local/overture_transport_hcm.parquet", 4_000, "demo_transport.parquet", "*"),
    "overture_places_hcm": ("data/local/overture_places_hcm.parquet", 4_000, "demo_places.parquet", "*"),
    "overture_admin_hcm": ("data/local/overture_admin_hcm.parquet", 500, "demo_admin.parquet", "*"),
    "overture_landuse_hcm": ("data/local/overture_landuse_hcm.parquet", 1_000, "demo_landuse.parquet", "*"),
}


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def column_type(field) -> object:
    """Map an Arrow field to the type names the gateway/UI use (`SourceMeta.columns`)."""
    t = str(field.type)
    if field.name == "geometry":
        return {"other": "geometry"}
    if t == "string":
        return "utf8"
    if t in ("double", "float"):
        return "float64"
    if t.startswith("int") or t.startswith("uint"):
        return "int64"
    if t == "bool":
        return "bool"
    return {"other": "struct" if t.startswith("struct") else t}


GEOMETRY_NAMES = {"Point": "point", "LineString": "line_string", "Polygon": "polygon"}


def describe(name: str, path: Path, public: str) -> dict:
    """`SourceMeta` + per-row-group boxes of one demo file, read from its Parquet footer (what the live gateway reports for a local file)."""
    pf = pq.ParquetFile(path)
    md = pf.metadata
    geo = json.loads((md.metadata or {})[b"geo"])
    gtypes = geo["columns"]["geometry"].get("geometry_types") or []
    groups = []
    for i in range(md.num_row_groups):
        rg = md.row_group(i)
        stat = {}
        for c in range(rg.num_columns):
            col = rg.column(c)
            if col.path_in_schema in ("bbox.xmin", "bbox.ymin", "bbox.xmax", "bbox.ymax") and col.statistics and col.statistics.has_min_max:
                stat[col.path_in_schema] = col.statistics
        box = None
        if len(stat) == 4:
            box = [stat["bbox.xmin"].min, stat["bbox.ymin"].min, stat["bbox.xmax"].max, stat["bbox.ymax"].max]
        groups.append({"index": i, "rows": rg.num_rows, "bytes": sum(rg.column(c).total_compressed_size for c in range(rg.num_columns)), "bbox": box})
    boxes = [g["bbox"] for g in groups if g["bbox"]]
    return {
        "id": name,
        "format": "GeoParquet",
        "size_bytes": path.stat().st_size,
        "rows": md.num_rows,
        "row_groups": md.num_row_groups,
        "crs": "OGC:CRS84",
        "crs_known": True,
        "geometry_column": "geometry",
        "geometry_type": GEOMETRY_NAMES.get(gtypes[0], "unknown") if len(gtypes) == 1 else "unknown",
        "bbox": [min(b[0] for b in boxes), min(b[1] for b in boxes), max(b[2] for b in boxes), max(b[3] for b in boxes)] if boxes else None,
        "has_covering": True,
        "covering_inferred": True,
        "modified": None,
        "columns": [{"name": f.name, "type": column_type(f)} for f in pf.schema_arrow],
        "file": public,
        "row_group_stats": groups,
    }


def catalog_entry(name: str, repo: Path, info: dict, bbox: list[float]) -> dict:
    """The live catalog entry of a dataset (from its committed manifest), pointed at the demo file."""
    m = json.loads((repo / "data" / "manifests" / f"{name}.json").read_text(encoding="utf-8"))
    note = (
        f"Public demo subset: only the window {bbox} (OGC:CRS84), rewritten with small row groups for browser range requests. "
        "A derived Overture database under the ODbL (share-alike); attribution as above."
    )
    if name == "overture_buildings_vn_south":
        note += " Flattened: id, name (names.primary), subtype, class, height, num_floors, facade_material, roof_material, roof_shape, has_parts, version, geometry, bbox; the other raw columns are dropped."
    return {
        "id": m["id"], "title": m["title"], "group": m["group"], "provider": m["provider"], "license": m["license"],
        "coverage": {"name": "Central Ho Chi Minh City (demo window)", "bbox": bbox},
        "crs": m["crs"], "geometry_type": m["geometry_type"], "format_normalized": m["format_normalized"],
        "source_ids": [name], "present": True,
        "files": [{"path": f"demo/{info['file']}", "size_bytes": info["size_bytes"], "rows": info["rows"]}],
        "raster": None, "quality": {"rows": info["rows"]}, "notes": (m.get("notes") or "") + " " + note,
    }


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--bbox", type=float, nargs=4, default=DEFAULT_BBOX)
    ap.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[2])
    args = ap.parse_args()
    x0, y0, x1, y1 = args.bbox
    args.out.mkdir(parents=True, exist_ok=True)

    con = duckdb.connect()
    con.execute("INSTALL spatial")
    con.execute("LOAD spatial")
    files = {}
    for name, (src, rg, public, projection) in SOURCES.items():
        source = (args.repo / src).as_posix()
        out = args.out / public
        tmp = Path(f"{out}.part")
        if tmp.exists():
            tmp.unlink()
        # same predicate as fetch_overture.py (covering-box overlap); no ORDER BY, so the spatial order of the source is kept
        con.execute(
            f"COPY (SELECT {projection} FROM read_parquet('{source}') WHERE bbox.xmin <= {x1} AND bbox.xmax >= {x0} AND bbox.ymin <= {y1} AND bbox.ymax >= {y0}) "
            f"TO '{tmp.as_posix()}' (FORMAT parquet, COMPRESSION zstd, ROW_GROUP_SIZE {rg})"
        )
        meta = pq.ParquetFile(tmp).metadata
        if b"geo" not in (meta.metadata or {}):
            raise SystemExit(f"{name}: output lost its GeoParquet metadata")
        tmp.replace(out)
        files[name] = {
            "file": public,
            "rows": meta.num_rows,
            "row_groups": meta.num_row_groups,
            "bytes": out.stat().st_size,
            "sha256": sha256(out),
            "derived_from": src,
        }
        print(name, files[name]["rows"], "rows", files[name]["bytes"] // 1024, "KiB")
    infos = {name: describe(name, args.out / SOURCES[name][2], SOURCES[name][2]) for name in SOURCES}
    (args.out / "sources.json").write_text(json.dumps({"sources": list(infos.values())}, ensure_ascii=False, separators=(",", ":")) + chr(10), encoding="utf-8")
    datasets = [catalog_entry(name, args.repo, infos[name], args.bbox) for name in SOURCES]
    attributions = sorted({d["license"]["attribution"] for d in datasets if d["license"].get("attribution")})
    (args.out / "catalog.json").write_text(json.dumps({"datasets": datasets, "attributions": attributions, "errors": []}, ensure_ascii=False, indent=1) + chr(10), encoding="utf-8")
    manifest = {
        "pack": "geofabric-public-demo-hcm-center",
        "built_at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "bbox_crs84": args.bbox,
        "overture_release": RELEASE,
        "license": "ODbL-1.0 (derived database; attribution required, see docs/DATA-LICENSES.md)",
        "files": files,
        "metadata": ["sources.json", "catalog.json"],
        "total_bytes": sum(f["bytes"] for f in files.values()),
    }
    (args.out / "pack.json").write_text(json.dumps(manifest, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print("total", manifest["total_bytes"] // 1024, "KiB")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
