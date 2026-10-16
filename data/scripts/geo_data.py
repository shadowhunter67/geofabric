"""Offline commands for reproducible GeoFabric dataset manifests."""

from __future__ import annotations

import argparse
import hashlib
import heapq
import json
import shutil
import sys
import tempfile
from collections import Counter
from pathlib import Path
from typing import Any, Iterator

import duckdb
import pyarrow as pa
import pyarrow.parquet as pq

from geoparquet_meta import geo_metadata, resolved_crs, wkb_info
from manifest import ROOT, dump, load, sha256_file

MANIFESTS = ROOT / "data" / "manifests"
GROUP_ORDER = {
    "base": 0,
    "buildings": 1,
    "transport": 2,
    "admin": 3,
    "places": 4,
    "raster": 5,
    "thematic": 6,
    "benchmark": 7,
}


def all_manifests(ids: list[str] | None = None) -> list[tuple[Path, dict[str, Any]]]:
    """Load every manifest first so commands cannot operate on invalid metadata."""

    manifests: list[tuple[Path, dict[str, Any]]] = []
    errors: list[str] = []

    for path in sorted(MANIFESTS.glob("*.json")):
        if path.name.startswith("_"):
            continue
        try:
            manifests.append((path, load(path)))
        except (OSError, ValueError, json.JSONDecodeError) as error:
            errors.append(str(error))

    if errors:
        raise ValueError("\n".join(errors))

    if ids is None:
        return manifests

    known_ids = {manifest["id"] for _, manifest in manifests}
    unknown_ids = sorted(set(ids) - known_ids)
    if unknown_ids:
        raise ValueError(f"unknown manifest: {', '.join(unknown_ids)}")

    return [(path, manifest) for path, manifest in manifests if manifest["id"] in ids]


def is_geoparquet(path: Path) -> bool:
    """Identify Parquet inputs because only they have GeoParquet verification rules."""

    return path.suffix.lower() in {".parquet", ".geoparquet"}


def _id_sort_key(value: Any) -> str:
    """Define digest ordering as plain Unicode string comparison of UTF-8 id values."""

    return str(value)


def _write_sorted_id_chunks(path: Path, batch_size: int = 50_000) -> list[Path] | None:
    """Create bounded sorted chunks so id digests do not retain an entire column."""

    parquet = pq.ParquetFile(path)
    if "id" not in parquet.schema_arrow.names:
        return None

    directory = Path(tempfile.mkdtemp(prefix="geofabric-id-digest-"))
    chunks: list[Path] = []
    try:
        for batch_number, batch in enumerate(
            parquet.iter_batches(batch_size=batch_size, columns=["id"])
        ):
            values = sorted(_id_sort_key(value) for value in batch.column(0).to_pylist())
            chunk_path = directory / f"{batch_number:08d}.txt"
            chunk_path.write_text("\n".join(values), encoding="utf-8", newline="\n")
            chunks.append(chunk_path)
    except Exception:
        shutil.rmtree(directory, ignore_errors=True)
        raise

    return chunks


def id_digest(path: Path) -> str | None:
    """Hash sorted id strings joined by newline using external merge-sort chunks.

    The canonical digest is SHA-256 of UTF-8 values sorted by normal Python string
    comparison, joined with exactly one LF between adjacent values and no final LF.
    """

    chunks = _write_sorted_id_chunks(path)
    if chunks is None:
        return None

    try:
        sources = [chunk.open("r", encoding="utf-8", newline="") for chunk in chunks]
        try:
            digest = hashlib.sha256()
            first = True
            for value in heapq.merge(*(source.read().splitlines() for source in sources)):
                if not first:
                    digest.update(b"\n")
                digest.update(value.encode("utf-8"))
                first = False
            return digest.hexdigest()
        finally:
            for source in sources:
                source.close()
    finally:
        if chunks:
            shutil.rmtree(chunks[0].parent, ignore_errors=True)


def _geometry_column(path: Path) -> str | None:
    """Find the declared primary geometry because column names are dataset-specific."""

    geo, primary_column = geo_metadata(path)
    if geo is None:
        return None
    return primary_column


def _bbox_from_metadata(path: Path) -> list[float] | None:
    """Use a GeoParquet bbox when supplied because it is the file's declared extent."""

    geo, primary_column = geo_metadata(path)
    if geo is None or primary_column is None:
        return None

    column = geo.get("columns", {}).get(primary_column)
    bbox = column.get("bbox") if isinstance(column, dict) else None
    if (
        isinstance(bbox, list)
        and len(bbox) >= 4
        and all(isinstance(value, (int, float)) for value in bbox[:4])
    ):
        return [bbox[0], bbox[1], bbox[2], bbox[3]]
    return None


def _bbox_from_bbox_column(path: Path) -> list[float] | None:
    """Use stored bbox structs before decoding WKB to minimize report-time work."""

    parquet = pq.ParquetFile(path)
    if "bbox" not in parquet.schema_arrow.names:
        return None

    minimum_x: float | None = None
    minimum_y: float | None = None
    maximum_x: float | None = None
    maximum_y: float | None = None

    aliases = (
        ("xmin", "minx", "x_min"),
        ("ymin", "miny", "y_min"),
        ("xmax", "maxx", "x_max"),
        ("ymax", "maxy", "y_max"),
    )

    for batch in parquet.iter_batches(batch_size=50_000, columns=["bbox"]):
        for value in batch.column(0).to_pylist():
            if not isinstance(value, dict):
                continue
            values = []
            for names in aliases:
                selected = next((value[name] for name in names if name in value), None)
                values.append(selected)
            if not all(isinstance(number, (int, float)) for number in values):
                continue
            xmin, ymin, xmax, ymax = values
            minimum_x = xmin if minimum_x is None else min(minimum_x, xmin)
            minimum_y = ymin if minimum_y is None else min(minimum_y, ymin)
            maximum_x = xmax if maximum_x is None else max(maximum_x, xmax)
            maximum_y = ymax if maximum_y is None else max(maximum_y, ymax)

    if None in (minimum_x, minimum_y, maximum_x, maximum_y):
        return None
    return [minimum_x, minimum_y, maximum_x, maximum_y]


def _spatial_connection() -> tuple[duckdb.DuckDBPyConnection | None, str]:
    """Probe only locally installed spatial support; reporting must never install it."""

    connection = duckdb.connect()
    try:
        connection.execute("LOAD spatial")
    except duckdb.Error as error:
        connection.close()
        return None, f"invalid_geometry_count is null: LOAD spatial failed locally ({error})."
    return connection, ""


def _invalid_count_for_batch(
    connection: duckdb.DuckDBPyConnection,
    geometry_values: list[bytes | None],
) -> int:
    """Ask DuckDB spatial about validity in bounded batches when it is available."""

    # DuckDB registers Arrow tables / DataFrames, not plain dicts (reviewer fix, 2026-10-16)
    connection.register("geometry_batch", pa.table({"geometry": pa.array(geometry_values, type=pa.binary())}))
    try:
        result = connection.execute(
            """
            SELECT count(*)
            FROM geometry_batch
            WHERE geometry IS NOT NULL
              AND NOT ST_IsValid(ST_GeomFromWKB(geometry))
            """
        ).fetchone()
        return int(result[0])
    finally:
        connection.unregister("geometry_batch")


def metrics(path: Path) -> dict[str, Any]:
    """Measure a GeoParquet file incrementally so reports work on large datasets."""

    parquet = pq.ParquetFile(path)
    schema = parquet.schema_arrow
    column_names = schema.names
    geometry_column = _geometry_column(path)
    null_counts = {name: 0 for name in column_names}
    geometry_types: Counter[str] = Counter()
    rows = 0
    null_geometry_count = 0
    empty_geometry_count = 0
    duplicate_id_count = 0
    id_values_seen: set[str] = set()
    bbox_from_wkb: list[float] | None = None

    connection, invalid_note = _spatial_connection()
    invalid_geometry_count: int | None = 0 if connection is not None else None

    columns = list(column_names)
    for batch in parquet.iter_batches(batch_size=50_000, columns=columns):
        rows += batch.num_rows
        for index, name in enumerate(columns):
            null_counts[name] += batch.column(index).null_count

        if "id" in columns:
            for value in batch.column(columns.index("id")).to_pylist():
                normalized = _id_sort_key(value)
                if normalized in id_values_seen:
                    duplicate_id_count += 1
                else:
                    id_values_seen.add(normalized)

        if geometry_column is None or geometry_column not in columns:
            continue

        geometry_values = batch.column(columns.index(geometry_column)).to_pylist()
        null_geometry_count += sum(value is None for value in geometry_values)

        for value in geometry_values:
            geometry_type, is_empty, value_bbox = wkb_info(value)
            if geometry_type is not None:
                geometry_types[geometry_type] += 1
            if is_empty:
                empty_geometry_count += 1
            if value_bbox is not None:
                if bbox_from_wkb is None:
                    bbox_from_wkb = value_bbox
                else:
                    bbox_from_wkb = [
                        min(bbox_from_wkb[0], value_bbox[0]),
                        min(bbox_from_wkb[1], value_bbox[1]),
                        max(bbox_from_wkb[2], value_bbox[2]),
                        max(bbox_from_wkb[3], value_bbox[3]),
                    ]

        if connection is not None:
            invalid_geometry_count = (invalid_geometry_count or 0) + _invalid_count_for_batch(
                connection,
                geometry_values,
            )

    if connection is not None:
        connection.close()

    row_group_sizes = [
        parquet.metadata.row_group(index).total_byte_size
        for index in range(parquet.metadata.num_row_groups)
    ]
    bbox = _bbox_from_metadata(path) or _bbox_from_bbox_column(path) or bbox_from_wkb
    notes = (
        "invalid_geometry_count was calculated with locally available DuckDB spatial."
        if connection is not None
        else invalid_note
    )

    return {
        "rows": rows,
        "geometry_types": dict(sorted(geometry_types.items())),
        "null_geometry_count": null_geometry_count,
        "empty_geometry_count": empty_geometry_count,
        "invalid_geometry_count": invalid_geometry_count,
        "per_column_null_ratio": {
            name: (null_counts[name] / rows if rows else 0.0) for name in column_names
        },
        "duplicate_id_count": duplicate_id_count if "id" in columns else 0,
        "crs": resolved_crs(path),
        "bbox": bbox,
        "file_size": path.stat().st_size,
        "row_groups": parquet.metadata.num_row_groups,
        "row_group_size_stats": {
            "min": min(row_group_sizes, default=0),
            "max": max(row_group_sizes, default=0),
            "mean": (sum(row_group_sizes) / len(row_group_sizes) if row_group_sizes else 0),
        },
        "notes": notes,
    }


def command_validate(_: argparse.Namespace) -> int:
    """Validate all manifests while collecting every invalid manifest error."""

    manifests: list[tuple[Path, dict[str, Any]]] = []
    errors: list[str] = []

    for path in sorted(MANIFESTS.glob("*.json")):
        if path.name.startswith("_"):
            continue
        try:
            manifests.append((path, load(path)))
        except (OSError, ValueError, json.JSONDecodeError) as error:
            errors.append(str(error))

    for _, manifest in manifests:
        print(f"OK {manifest['id']}")
    for error in errors:
        print(f"ERROR: {error}", file=sys.stderr)

    return 1 if errors else 0


def command_list(_: argparse.Namespace) -> int:
    """List manifest facts without touching local data contents."""

    print("id\tgroup\tprovider\tlicense\trows\tsize\tpresent-locally")
    for _, manifest in all_manifests():
        files = manifest["files"]
        present = bool(files) and all((ROOT / file_info["path"]).is_file() for file_info in files)
        rows = sum(file_info["rows"] or 0 for file_info in files)
        size = sum(file_info["size_bytes"] for file_info in files)
        print(
            f"{manifest['id']}\t{manifest['group']}\t{manifest['provider']}\t"
            f"{manifest['license']['name']}\t{rows}\t{size}\t{'yes' if present else 'no'}"
        )
    return 0


def command_verify(args: argparse.Namespace) -> int:
    """Verify sealed local files without normalizing, repairing, or changing them."""

    failed = False
    for _, manifest in all_manifests(args.ids):
        for file_info in manifest["files"]:
            path = ROOT / file_info["path"]
            result = "OK"

            if not path.is_file():
                result = "MISSING"
            elif path.stat().st_size != file_info["size_bytes"]:
                result = "MISMATCH (size)"
            elif sha256_file(path).lower() != file_info["sha256"].lower():
                result = "MISMATCH (sha256)"
            elif is_geoparquet(path):
                crs = resolved_crs(path)
                if crs == "NOGEO" and manifest["format_normalized"] == "GeoParquet":
                    result = "MISMATCH (missing geo metadata)"
                elif crs != "NOGEO" and crs != manifest["crs"]:
                    result = f"MISMATCH (crs {crs})"
                elif (
                    file_info["rows"] is not None
                    and pq.ParquetFile(path).metadata.num_rows != file_info["rows"]
                ):
                    result = "MISMATCH (rows)"
                elif file_info["id_digest"] is not None:
                    actual_digest = id_digest(path)
                    if actual_digest is None or actual_digest.lower() != file_info["id_digest"].lower():
                        result = "MISMATCH (id_digest)"

            print(f"{manifest['id']} {file_info['path']}: {result}")
            failed = failed or result != "OK"

    return 1 if failed else 0


def command_report(args: argparse.Namespace) -> int:
    """Write read-only measurements for existing local GeoParquet files."""

    quality_directory = ROOT / "data" / "quality"
    quality_directory.mkdir(parents=True, exist_ok=True)

    for _, manifest in all_manifests(args.ids):
        reports = [
            metrics(ROOT / file_info["path"])
            for file_info in manifest["files"]
            if is_geoparquet(ROOT / file_info["path"]) and (ROOT / file_info["path"]).is_file()
        ]
        output_path = quality_directory / f"{manifest['id']}.json"
        dump(
            output_path,
            {
                "id": manifest["id"],
                "files": reports,
                "notes": "Reports are read-only measurements; dataset files were not modified.",
            },
        )
        print(f"WROTE data/quality/{manifest['id']}.json")

    return 0


def command_seal(args: argparse.Namespace) -> int:
    """Seal measured local files so later verify runs detect data drift."""

    manifest_path, manifest = all_manifests([args.id])[0]
    files: list[dict[str, Any]] = []

    for supplied_path in args.paths:
        path = Path(supplied_path)
        if not path.is_absolute():
            path = ROOT / path
        if not path.is_file():
            raise ValueError(f"{manifest['id']}: files.path: missing {supplied_path}")

        try:
            relative_path = path.resolve().relative_to(ROOT.resolve()).as_posix()
        except ValueError as error:
            raise ValueError(f"{manifest['id']}: files.path: must be inside repository root") from error

        if is_geoparquet(path):
            parquet = pq.ParquetFile(path)
            rows: int | None = parquet.metadata.num_rows
            digest = id_digest(path)
        else:
            rows = None
            digest = None

        files.append(
            {
                "path": relative_path,
                "size_bytes": path.stat().st_size,
                "sha256": sha256_file(path),
                "rows": rows,
                "id_digest": digest,
            }
        )

    manifest["files"] = files
    dump(manifest_path, manifest)
    return 0


def command_bootstrap(args: argparse.Namespace) -> int:
    """Show deterministic reproduction commands while M1 intentionally stays offline."""

    if args.run:
        raise ValueError("not implemented in M1")

    matching = [
        manifest
        for _, manifest in all_manifests()
        if manifest["coverage"]["name"] == args.region
    ]
    matching.sort(key=lambda manifest: (GROUP_ORDER[manifest["group"]], manifest["id"]))

    for manifest in matching:
        command = " ".join([manifest["fetch"]["script"], *manifest["fetch"]["args"]]).rstrip()
        print(f"{manifest['id']}: {command}")

    return 0


def build_parser() -> argparse.ArgumentParser:
    """Define the small offline CLI once so argument parsing has one source of truth."""

    parser = argparse.ArgumentParser()
    subcommands = parser.add_subparsers(dest="command", required=True)

    validate_parser = subcommands.add_parser("validate")
    validate_parser.set_defaults(func=command_validate)

    list_parser = subcommands.add_parser("list")
    list_parser.set_defaults(func=command_list)

    for name, function in (("verify", command_verify), ("report", command_report)):
        command_parser = subcommands.add_parser(name)
        command_parser.add_argument("ids", nargs="*")
        command_parser.set_defaults(func=function)

    seal_parser = subcommands.add_parser("seal")
    seal_parser.add_argument("id")
    seal_parser.add_argument("paths", nargs="+")
    seal_parser.set_defaults(func=command_seal)

    bootstrap_parser = subcommands.add_parser("bootstrap")
    bootstrap_parser.add_argument("region")
    bootstrap_parser.add_argument("--run", action="store_true")
    bootstrap_parser.set_defaults(func=command_bootstrap)

    return parser


def main() -> int:
    """Return a non-zero status for every user-visible validation or I/O failure."""

    parser = build_parser()
    args = parser.parse_args()
    try:
        return args.func(args)
    except (OSError, ValueError, json.JSONDecodeError, duckdb.Error, pq.ArrowInvalid) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
