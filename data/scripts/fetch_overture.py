"""Fetch normalized Overture subsets while preserving spatially sorted row order.

Only the first road-surface, speed-limit, and source record is retained where
applicable. Other nested Overture columns are deliberately dropped so browser
and web consumers receive a compact flat schema.
"""

from __future__ import annotations

import argparse
import json
import os
import threading
import time
from dataclasses import dataclass
from pathlib import Path

import duckdb
import pyarrow.parquet as pq

from geoparquet_meta import resolved_crs

DEFAULT_BBOX = [106.35, 10.35, 107.05, 11.25]
DEFAULT_RELEASE = "2026-09-23.1"


@dataclass(frozen=True)
class Preset:
    """Describe a stable normalized subset because consumers require flat columns."""

    theme: str
    type_name: str
    extra_where: str
    select: tuple[str, ...]
    row_group_size: int
    description: str


PRESETS = {
    "transport": Preset(
        theme="transportation",
        type_name="segment",
        extra_where="",
        select=(
            "id",
            "names.primary AS name",
            "subtype",
            "class",
            "subclass",
            "road_surface[1].value AS surface",
            "speed_limits[1].max_speed.value AS max_speed",
            "speed_limits[1].max_speed.unit AS max_speed_unit",
            "sources[1].dataset AS source_dataset",
            "version",
            "geometry",
            "bbox",
        ),
        row_group_size=50_000,
        description=(
            "Normalized flat schema. Retains only road_surface[1], speed_limits[1], "
            "and sources[1]; additional rules and sources are dropped. Drops nested "
            "connectors, routes, destinations, prohibited_transitions, "
            "access_restrictions, road_surface records after the first, speed_limits "
            "after the first, sources after the first, and all other raw Overture "
            "columns not listed in the selected normalized schema."
        ),
    ),
    "places": Preset(
        theme="places",
        type_name="place",
        extra_where="",
        select=(
            "id",
            "names.primary AS name",
            "basic_category AS category",
            "taxonomy.primary AS taxonomy",
            "confidence",
            "addresses[1].freeform AS address",
            "addresses[1].locality AS locality",
            "operating_status",
            "sources[1].dataset AS source_dataset",
            "version",
            "geometry",
            "bbox",
        ),
        row_group_size=50_000,
        description=(
            "Normalized flat schema: id, name (names.primary), category (basic_category), taxonomy (taxonomy.primary), "
            "confidence, address (addresses[1].freeform), locality (addresses[1].locality), operating_status, "
            "source_dataset (sources[1].dataset), version, geometry, bbox. Only the first address and the first source "
            "record are kept. Dropped: websites, emails, socials, phones, brand, other names, taxonomy hierarchy/alternates "
            "and every other raw column."
        ),
    ),
    "admin": Preset(
        theme="divisions",
        type_name="division_area",
        extra_where="country = 'VN'",
        select=(
            "id",
            "names.primary AS name",
            "subtype",
            "admin_level",
            "class",
            "region",
            "division_id",
            "is_land",
            "sources[1].dataset AS source_dataset",
            "version",
            "geometry",
            "bbox",
        ),
        row_group_size=10_000,
        description=(
            "Normalized flat schema restricted to country = 'VN': id, name (names.primary), subtype, admin_level, class, "
            "region, division_id, is_land, source_dataset (sources[1].dataset), version, geometry, bbox. Overture holds "
            "provinces (region), wards and communes (locality), microhoods and neighborhoods here; there is NO district tier "
            "and none is inferred. Land and territorial-water versions of one unit are separate rows (is_land). Dropped: "
            "other names, other source records and every other raw column."
        ),
    ),
    "landuse": Preset(
        theme="base",
        type_name="land_use",
        # Drops continental/oceanic-scale protected-area polygons (one 347 x 26 degree "marine national monument"
        # overlaps the bbox and would paint the whole screen): extent wider than 5 degrees is not land use of this region.
        extra_where="bbox.xmax - bbox.xmin <= 5 AND bbox.ymax - bbox.ymin <= 5",
        select=(
            "id",
            "names.primary AS name",
            "subtype",
            "class",
            "surface",
            "sources[1].dataset AS source_dataset",
            "version",
            "geometry",
            "bbox",
        ),
        row_group_size=50_000,
        description=(
            "Normalized flat schema: id, name (names.primary), subtype, class, surface, source_dataset "
            "(sources[1].dataset: only the first source record is kept), version, geometry, bbox. Dropped: the other "
            "source records, source_tags, wikidata, level, elevation and every other raw column. Rows whose bbox is "
            "wider or taller than 5 degrees are excluded (ocean-scale protected-area polygons)."
        ),
    ),
}


def sql_string(value: str) -> str:
    """Quote SQL literals because paths and release strings must not alter SQL."""

    return "'" + value.replace("'", "''") + "'"


def build_sql(preset: Preset, bbox: list[float], release: str, out_tmp: Path) -> str:
    """Build deterministic COPY SQL without ORDER BY to retain Overture row order."""

    x0, y0, x1, y1 = bbox
    source = (
        "s3://overturemaps-us-west-2/release/"
        f"{release}/theme={preset.theme}/type={preset.type_name}/*"
    )
    predicates = [
        f"bbox.xmin <= {x1}",
        f"bbox.xmax >= {x0}",
        f"bbox.ymin <= {y1}",
        f"bbox.ymax >= {y0}",
    ]
    if preset.extra_where:
        predicates.append(preset.extra_where)

    return (
        "COPY (\n"
        f"  SELECT {', '.join(preset.select)}\n"
        f"  FROM read_parquet({sql_string(source)}, hive_partitioning=true)\n"
        f"  WHERE {' AND '.join(predicates)}\n"
        f") TO {sql_string(out_tmp.as_posix())} "
        f"(FORMAT parquet, COMPRESSION zstd, ROW_GROUP_SIZE {preset.row_group_size})"
    )


def validate_bbox(bbox: list[float]) -> list[float]:
    """Reject ambiguous extents because Overture bbox predicates require ordered CRS84."""

    x0, y0, x1, y1 = bbox
    if not -180 <= x0 < x1 <= 180 or not -90 <= y0 < y1 <= 90:
        raise ValueError("bbox must be ordered longitude/latitude coordinates")
    return bbox


def verify_geoparquet(path: Path) -> None:
    """Reject files without explicit GeoParquet CRS metadata before publication."""

    metadata = pq.ParquetFile(path).metadata.metadata or {}
    if b"geo" not in metadata:
        raise ValueError("output is missing GeoParquet geo metadata")
    if resolved_crs(path) != "OGC:CRS84":
        raise ValueError(f"output CRS is {resolved_crs(path)}, expected OGC:CRS84")


def main() -> int:
    """Fetch one preset atomically so interrupted runs cannot appear complete."""

    parser = argparse.ArgumentParser()
    parser.add_argument("--preset", choices=sorted(PRESETS), required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--bbox", type=float, nargs=4, default=DEFAULT_BBOX)
    parser.add_argument("--release", default=DEFAULT_RELEASE)
    parser.add_argument("--max-gb", type=float, default=3.0)
    parser.add_argument("--threads", type=int, default=4)
    args = parser.parse_args()

    if args.max_gb <= 0 or args.threads <= 0:
        raise ValueError("--max-gb and --threads must be positive")

    bbox = validate_bbox(args.bbox)
    preset = PRESETS[args.preset]
    output = args.out
    temporary = Path(f"{output}.part")
    output.parent.mkdir(parents=True, exist_ok=True)
    if temporary.exists():
        temporary.unlink()

    connection = duckdb.connect()
    connection.execute("INSTALL httpfs")
    connection.execute("LOAD httpfs")
    connection.execute("INSTALL spatial")
    connection.execute("LOAD spatial")
    connection.execute("SET s3_region='us-west-2'")
    connection.execute(f"SET threads={args.threads}")

    stopped = threading.Event()
    exceeded = threading.Event()

    def watchdog() -> None:
        """Interrupt oversized writes because the configured limit is a hard safety cap."""

        while not stopped.wait(1):
            if temporary.exists() and temporary.stat().st_size > args.max_gb * 1_000_000_000:
                exceeded.set()
                connection.interrupt()
                return

    thread = threading.Thread(target=watchdog, daemon=True)
    thread.start()
    started = time.monotonic()

    try:
        connection.execute(build_sql(preset, bbox, args.release, temporary))
        if exceeded.is_set():
            raise ValueError(f"output exceeds {args.max_gb} GB")
        verify_geoparquet(temporary)
        os.replace(temporary, output)
    except Exception:
        if temporary.exists():
            temporary.unlink()
        raise
    finally:
        stopped.set()
        connection.close()

    parquet = pq.ParquetFile(output)
    print(
        json.dumps(
            {
                "bbox": bbox,
                "preset": args.preset,
                "release": args.release,
                "rows": parquet.metadata.num_rows,
                "seconds": round(time.monotonic() - started, 1),
                "size_bytes": output.stat().st_size,
            },
            ensure_ascii=False,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
