"""Estimate pilot Overture reads from Parquet row-group metadata only."""

from __future__ import annotations

import argparse
import json
import sys
import time
from collections.abc import Iterable, Mapping, Sequence
from pathlib import Path
from typing import Any

import duckdb

from fetch_overture import DEFAULT_RELEASE

DOWNLOAD_GATE_BYTES = 3_000_000_000
DEFAULT_REGIONS = Path(__file__).parents[1] / "pilot" / "regions.json"
DEFAULT_OUTPUT = Path(__file__).parents[1] / "quality" / "pilot_estimate.json"

# Project-wide CRS84 sanity box for Vietnam, including its surrounding sea.
NATIONAL_ADMIN_BBOX = [100.0, 6.0, 118.0, 25.0]

THEME_PATHS = {
    "buildings": "theme=buildings/type=building/*",
    "roads": "theme=transportation/type=segment/*",
    "places": "theme=places/type=place/*",
    "land_use": "theme=base/type=land_use/*",
    "admin": "theme=divisions/type=division_area/*",
}

THEME_EXPLANATIONS = {
    "buildings": (
        "An upper bound from Parquet row groups whose bbox statistics intersect the "
        "five pilot regions, conservatively including groups without usable statistics."
    ),
    "roads": (
        "An upper bound from Parquet row groups whose bbox statistics intersect the "
        "five pilot regions, conservatively including groups without usable statistics."
    ),
    "places": (
        "An upper bound from Parquet row groups whose bbox statistics intersect the "
        "five pilot regions, conservatively including groups without usable statistics."
    ),
    "land_use": (
        "An upper bound from Parquet row groups whose bbox statistics intersect the "
        "five pilot regions, conservatively including groups without usable statistics."
    ),
    "admin": (
        "An upper bound from Parquet row groups intersecting the national extent of "
        "the Vietnam sanity box because country cannot be filtered from footers."
    ),
}

# Keep this explicit: it is both the estimator's byte-accounting contract and a
# reviewable list of the source columns retained by the normalized presets.
SELECTED_TOP_LEVEL_COLUMNS = {
    "buildings": frozenset(
        {
            "id",
            "names",
            "subtype",
            "class",
            "height",
            "num_floors",
            "has_parts",
            "is_underground",
            "sources",
            "version",
            "geometry",
            "bbox",
        }
    ),
    "roads": frozenset(
        {
            "id",
            "names",
            "subtype",
            "class",
            "subclass",
            "road_surface",
            "speed_limits",
            "sources",
            "version",
            "geometry",
            "bbox",
        }
    ),
    "places": frozenset(
        {
            "id",
            "names",
            "basic_category",
            "taxonomy",
            "confidence",
            "addresses",
            "operating_status",
            "sources",
            "version",
            "geometry",
            "bbox",
        }
    ),
    "land_use": frozenset(
        {
            "id",
            "names",
            "subtype",
            "class",
            "surface",
            "sources",
            "version",
            "geometry",
            "bbox",
        }
    ),
    "admin": frozenset(
        {
            "id",
            "names",
            "subtype",
            "admin_level",
            "class",
            "region",
            "division_id",
            "is_land",
            "sources",
            "version",
            "geometry",
            "bbox",
        }
    ),
}


def row_group_may_intersect(
    row_group: Mapping[str, Any], bbox: Sequence[float]
) -> tuple[bool, bool]:
    """Return (may intersect, lacks usable stats) for one row group."""

    required = {
        "bbox.xmin": row_group.get("xmin_min"),
        "bbox.ymin": row_group.get("ymin_min"),
        "bbox.xmax": row_group.get("xmax_max"),
        "bbox.ymax": row_group.get("ymax_max"),
    }
    if any(value is None for value in required.values()):
        return True, True

    west, south, east, north = bbox
    return (
        bool(
            required["bbox.xmin"] <= east
            and required["bbox.xmax"] >= west
            and required["bbox.ymin"] <= north
            and required["bbox.ymax"] >= south
        ),
        False,
    )


def touched_row_groups(
    row_groups: Iterable[Mapping[str, Any]], bbox: Sequence[float] | None
) -> tuple[list[Mapping[str, Any]], set[tuple[str, int]]]:
    """Select possible intersections and identify conservative inclusions."""

    touched: list[Mapping[str, Any]] = []
    without_stats: set[tuple[str, int]] = set()
    for row_group in row_groups:
        identity = (str(row_group["file_name"]), int(row_group["row_group_id"]))
        intersects, missing = (
            (True, False)
            if bbox is None
            else row_group_may_intersect(row_group, bbox)
        )
        if intersects:
            touched.append(row_group)
            if missing:
                without_stats.add(identity)
    return touched, without_stats


def summarize_row_groups(
    touched: Iterable[Mapping[str, Any]],
    row_groups_total: int,
    without_stats: Iterable[tuple[str, int]] = (),
) -> dict[str, int]:
    """Calculate a conservative report from a deduplicated row-group collection."""

    unique = {
        (str(group["file_name"]), int(group["row_group_id"])): group for group in touched
    }
    return {
        "rows_upper_bound": sum(int(group["row_group_num_rows"]) for group in unique.values()),
        "row_groups_touched": len(unique),
        "row_groups_total": row_groups_total,
        "files_touched": len({str(group["file_name"]) for group in unique.values()}),
        "compressed_bytes": sum(int(group["compressed_bytes"]) for group in unique.values()),
        "uncompressed_bytes": sum(
            int(group["uncompressed_bytes"]) for group in unique.values()
        ),
        "row_groups_without_stats": len(set(without_stats) & set(unique)),
    }


def summarize_regions(
    row_groups: Sequence[Mapping[str, Any]],
    regions: Mapping[str, Sequence[float]],
    *,
    theme_wide: bool = False,
) -> tuple[dict[str, dict[str, int]], dict[str, int]]:
    """Report each region plus a row-group-deduplicated union over all regions."""

    reports: dict[str, dict[str, int]] = {}
    union: dict[tuple[str, int], Mapping[str, Any]] = {}
    union_without_stats: set[tuple[str, int]] = set()
    for name, bbox in regions.items():
        touched, without_stats = touched_row_groups(row_groups, None if theme_wide else bbox)
        reports[name] = summarize_row_groups(touched, len(row_groups), without_stats)
        for group in touched:
            union[(str(group["file_name"]), int(group["row_group_id"]))] = group
        union_without_stats.update(without_stats)
    return reports, summarize_row_groups(
        union.values(), len(row_groups), union_without_stats
    )


def gate_exit_code(estimated_bytes: int, *, partial: bool = False) -> int:
    """Return the approved gate status; partial dry runs cannot apply the gate."""

    return 0 if partial or estimated_bytes <= DOWNLOAD_GATE_BYTES else 3


def bbox_stats_failure(report: Mapping[str, int], *, theme_wide: bool = False) -> bool:
    """Return whether a spatial estimate has multiple hits and no usable bbox stats."""

    touched = report["row_groups_touched"]
    return (
        not theme_wide
        and touched > 1
        and report["row_groups_without_stats"] == touched
    )


def _sql_string(value: str) -> str:
    """Quote a SQL literal used for an operator-selected release or S3 path."""

    return "'" + value.replace("'", "''") + "'"


def metadata_aggregation_sql(relation_name: str) -> str:
    """Build the row-group aggregation for a metadata relation or table function."""

    return f"""
WITH normalized_metadata AS (
    SELECT
        file_name,
        row_group_id,
        row_group_num_rows,
        regexp_replace(trim(path_in_schema), '\\s*,\\s*', '.', 'g') AS normalized_path,
        stats_min,
        stats_max,
        stats_min_value,
        stats_max_value,
        total_compressed_size,
        total_uncompressed_size
    FROM {relation_name}
), classified_metadata AS (
    SELECT
        *,
        split_part(normalized_path, '.', 1) AS top_level_column,
        list_contains(?::VARCHAR[], split_part(normalized_path, '.', 1)) AS selected
    FROM normalized_metadata
)
SELECT
    file_name,
    row_group_id,
    max(row_group_num_rows) AS row_group_num_rows,
    min(TRY_CAST(coalesce(stats_min, stats_min_value) AS DOUBLE))
        FILTER (WHERE normalized_path = 'bbox.xmin') AS xmin_min,
    min(TRY_CAST(coalesce(stats_min, stats_min_value) AS DOUBLE))
        FILTER (WHERE normalized_path = 'bbox.ymin') AS ymin_min,
    max(TRY_CAST(coalesce(stats_max, stats_max_value) AS DOUBLE))
        FILTER (WHERE normalized_path = 'bbox.xmax') AS xmax_max,
    max(TRY_CAST(coalesce(stats_max, stats_max_value) AS DOUBLE))
        FILTER (WHERE normalized_path = 'bbox.ymax') AS ymax_max,
    sum(CASE WHEN selected THEN coalesce(total_compressed_size, 0) ELSE 0 END)
        AS compressed_bytes,
    sum(CASE WHEN selected THEN coalesce(total_uncompressed_size, 0) ELSE 0 END)
        AS uncompressed_bytes
FROM classified_metadata
GROUP BY file_name, row_group_id
ORDER BY file_name, row_group_id
"""


def fetch_row_group_metadata(
    connection: duckdb.DuckDBPyConnection,
    source_glob: str,
    max_files: int | None,
    selected_columns: set[str] | frozenset[str],
) -> list[dict[str, Any]]:
    """Aggregate footer metadata in DuckDB and return one record per row group."""

    source: str
    if max_files is None:
        source = _sql_string(source_glob)
    else:
        files = [
            row[0]
            for row in connection.execute(
                "SELECT file FROM glob(?) ORDER BY file LIMIT ?", [source_glob, max_files]
            ).fetchall()
        ]
        if not files:
            return []
        source = "[" + ", ".join(_sql_string(path) for path in files) + "]"

    result = connection.execute(
        metadata_aggregation_sql(f"parquet_metadata({source})"),
        [sorted(selected_columns)],
    )
    names = [item[0] for item in result.description]
    return [dict(zip(names, row, strict=True)) for row in result.fetchall()]


def admin_counts(connection: duckdb.DuckDBPyConnection, source_glob: str) -> dict[str, int]:
    """Run the sole optional data-page query: Vietnamese admin counts by subtype."""

    rows = connection.execute(
        "SELECT subtype, count(*) FROM read_parquet(?) "
        "WHERE country = 'VN' GROUP BY subtype ORDER BY subtype",
        [source_glob],
    ).fetchall()
    return {str(subtype): int(count) for subtype, count in rows}


def load_regions(path: Path) -> tuple[str, dict[str, list[float]]]:
    """Load the pinned release and ordered CRS84 pilot windows."""

    value = json.loads(path.read_text(encoding="utf-8"))
    release = value.get("overture_release", DEFAULT_RELEASE)
    regions = {name: list(bbox) for name, bbox in value["regions"].items()}
    return release, regions


def print_table(report: Mapping[str, Any]) -> None:
    """Print compact per-region/theme rows followed by union totals."""

    print("region theme rows row_groups files compressed_bytes uncompressed_bytes no_stats")
    for region, region_reports in report["regions"].items():
        for theme, item in region_reports.items():
            print(
                f"{region} {theme} {item['rows_upper_bound']} "
                f"{item['row_groups_touched']}/{item['row_groups_total']} "
                f"{item['files_touched']} {item['compressed_bytes']} "
                f"{item['uncompressed_bytes']} {item['row_groups_without_stats']}"
            )
    print("theme union_compressed_bytes")
    for theme in report["theme_unions"]:
        print(f"{theme} {report['theme_unions'][theme]['compressed_bytes']}")
    print(f"estimated_pilot_download_bytes {report['estimated_pilot_download_bytes']}")


def main(argv: Sequence[str] | None = None) -> int:
    """Acquire footer metadata, write the estimate, and enforce the download gate."""

    parser = argparse.ArgumentParser()
    parser.add_argument("--regions", type=Path, default=DEFAULT_REGIONS)
    parser.add_argument("--release")
    parser.add_argument("--out", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--max-files", type=int)
    parser.add_argument("--themes", default=",".join(THEME_PATHS))
    parser.add_argument("--memory-limit", default="2GB")
    parser.add_argument("--admin-count", action="store_true")
    args = parser.parse_args(argv)
    if args.max_files is not None and args.max_files <= 0:
        parser.error("--max-files must be positive")
    themes = list(
        dict.fromkeys(item.strip() for item in args.themes.split(",") if item.strip())
    )
    if not themes:
        parser.error("--themes must select at least one theme")
    unknown_themes = [theme for theme in themes if theme not in THEME_PATHS]
    if unknown_themes:
        parser.error("unknown --themes value(s): " + ", ".join(unknown_themes))

    region_release, regions = load_regions(args.regions)
    release = args.release or region_release
    partial = args.max_files is not None
    connection = duckdb.connect()
    try:
        connection.execute("SET memory_limit = ?", [args.memory_limit])
        connection.execute("INSTALL httpfs")
        connection.execute("LOAD httpfs")
        connection.execute("SET s3_region='us-west-2'")
        per_region = {name: {} for name in regions}
        theme_unions: dict[str, dict[str, int]] = {}
        themes_without_bbox_stats: list[str] = []
        counts: dict[str, int] | None = None
        for theme in themes:
            started = time.perf_counter()
            print(f"START {theme}", file=sys.stderr, flush=True)
            relative_path = THEME_PATHS[theme]
            source = f"s3://overturemaps-us-west-2/release/{release}/{relative_path}"
            groups = fetch_row_group_metadata(
                connection,
                source,
                args.max_files,
                SELECTED_TOP_LEVEL_COLUMNS[theme],
            )
            estimate_regions = (
                {"national": NATIONAL_ADMIN_BBOX} if theme == "admin" else regions
            )
            reports, union = summarize_regions(groups, estimate_regions)
            for region, item in reports.items():
                per_region.setdefault(region, {})[theme] = item
            theme_unions[theme] = union
            if bbox_stats_failure(union):
                themes_without_bbox_stats.append(theme)
            if theme == "admin" and args.admin_count:
                counts = admin_counts(connection, source)
            elapsed = time.perf_counter() - started
            print(f"FINISH {theme} elapsed_seconds={elapsed:.3f}", file=sys.stderr, flush=True)
    finally:
        connection.close()

    estimate = sum(item["compressed_bytes"] for item in theme_unions.values())
    output: dict[str, Any] = {
        "schema": 1,
        "release": release,
        "partial": partial,
        "regions": per_region,
        "theme_unions": theme_unions,
        "explanations": {theme: THEME_EXPLANATIONS[theme] for theme in themes},
        "estimated_pilot_download_bytes": estimate,
        "download_gate_bytes": DOWNLOAD_GATE_BYTES,
    }
    if counts is not None:
        output["admin_vn_counts_by_subtype"] = counts

    print_table(output)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(
        json.dumps(output, ensure_ascii=False, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    if themes_without_bbox_stats:
        print(
            "ERROR: bbox statistics were not found for all touched row groups in "
            + ", ".join(themes_without_bbox_stats)
        )
        return 4
    status = gate_exit_code(estimate, partial=partial)
    if status == 3:
        print(f"STOP: estimated_pilot_download_bytes={estimate}")
    return status


if __name__ == "__main__":
    raise SystemExit(main())
