"""Build deterministic adaptive-quadtree GeoParquet shards from Parquet input."""

from __future__ import annotations

import argparse
from dataclasses import asdict, dataclass
import hashlib
import json
import math
import os
from pathlib import Path
import re
import statistics
import time
from typing import Any, Mapping, Sequence

import duckdb

from fetch_overture import verify_geoparquet
import quadtree_partition as quadtree


@dataclass(frozen=True)
class DatasetConfig:
    dataset_id: str
    theme: str
    source_release: str
    license: str
    crs: str = "OGC:CRS84"
    target_bytes: int = 25_000_000
    hard_cap_bytes: int = 45_000_000
    oversize_extent_deg: float | None = None
    min_zoom: int = 0
    geometry_type: str | None = None
    row_group_size: int = 25_000
    weight_geom_ratio: float = 0.55
    weight_attr_bytes: int = 40

    def __post_init__(self) -> None:
        if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]*", self.dataset_id):
            raise ValueError("dataset_id must be a safe non-empty path component")
        if not self.theme:
            raise ValueError("theme must not be empty")
        if not self.source_release or "latest" in self.source_release.casefold():
            raise ValueError("source_release must be pinned, never 'latest'")
        if not self.license:
            raise ValueError("license must not be empty")
        if self.crs != "OGC:CRS84":
            raise ValueError("crs must be OGC:CRS84")
        for name in ("target_bytes", "hard_cap_bytes", "row_group_size"):
            value = getattr(self, name)
            if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
                raise ValueError(f"{name} must be a positive integer")
        if not 0 <= self.min_zoom <= 22:
            raise ValueError("min_zoom must be between 0 and 22")
        if self.weight_geom_ratio < 0 or not math.isfinite(self.weight_geom_ratio):
            raise ValueError("weight_geom_ratio must be finite and non-negative")
        if self.weight_attr_bytes < 0:
            raise ValueError("weight_attr_bytes must be non-negative")
        if self.oversize_extent_deg is not None:
            quadtree.oversize_split(0, self.oversize_extent_deg)


def adjust_target(target_bytes: int, actual_max_bytes: int, hard_cap_bytes: int) -> int:
    """Scale a planning target down with ten percent safety margin, never up."""

    for name, value in (
        ("target_bytes", target_bytes),
        ("actual_max_bytes", actual_max_bytes),
        ("hard_cap_bytes", hard_cap_bytes),
    ):
        if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
            raise ValueError(f"{name} must be a positive integer")
    scaled = math.floor(target_bytes * hard_cap_bytes / actual_max_bytes * 0.9)
    return min(target_bytes, max(1, scaled))


def _sql_string(value: str | Path) -> str:
    return "'" + str(value).replace("\\", "/").replace("'", "''") + "'"


def _identifier(value: str) -> str:
    return '"' + value.replace('"', '""') + '"'


def _source_sql(source: str | Path | Sequence[str | Path]) -> str:
    if isinstance(source, (str, Path)):
        return _sql_string(source)
    if not source:
        raise ValueError("source file list must not be empty")
    return "[" + ", ".join(_sql_string(item) for item in source) + "]"


def _read_parquet(source: str | Path | Sequence[str | Path]) -> str:
    return f"read_parquet({_source_sql(source)})"


def _load_spatial(
    connection: duckdb.DuckDBPyConnection,
    *,
    memory_limit: str = "6GB",
    temp_dir: Path | None = None,
    threads: int = 1,
) -> None:
    if not isinstance(memory_limit, str) or not memory_limit.strip():
        raise ValueError("memory_limit must be a non-empty DuckDB memory size")
    if isinstance(threads, bool) or not isinstance(threads, int) or threads <= 0:
        raise ValueError("threads must be a positive integer")
    connection.execute(f"SET memory_limit = {_sql_string(memory_limit)}")
    if temp_dir is not None:
        connection.execute(f"SET temp_directory = {_sql_string(temp_dir)}")
    connection.execute(f"SET threads = {threads}")
    connection.execute("LOAD spatial")
    connection.execute("SET preserve_insertion_order = true")


def _fine_path(ix: int, iy: int, *, prefix: str = "q") -> str:
    limit = 1 << quadtree.MAX_DEPTH
    if prefix not in ("q", "L"):
        raise ValueError("prefix must be 'q' or 'L'")
    if not 0 <= ix < limit or not 0 <= iy < limit:
        raise ValueError("fine-cell indices are outside ROOT")
    digits = []
    for bit in range(quadtree.MAX_DEPTH - 1, -1, -1):
        digits.append(str(((ix >> bit) & 1) + 2 * ((iy >> bit) & 1)))
    return prefix + "".join(digits)


def _morton(ix: int, iy: int) -> int:
    return int(_fine_path(ix, iy)[1:], 4)


def _centre_sql() -> tuple[str, str]:
    return (
        '(("bbox".xmin + "bbox".xmax) / 2.0)',
        '(("bbox".ymin + "bbox".ymax) / 2.0)',
    )


def _index_sql(coordinate: str, origin: float) -> str:
    cells = 1 << quadtree.MAX_DEPTH
    width = (quadtree.ROOT[2] - quadtree.ROOT[0]) / cells
    return (
        f"CAST(least({cells - 1}, greatest(0, "
        f"floor(({coordinate} - {origin}) / {width}))) AS INTEGER)"
    )


def _group_sql(threshold: float | None) -> str:
    if threshold is None or threshold == 0:
        return "'q'"
    return (
        "CASE WHEN greatest(\"bbox\".xmax - \"bbox\".xmin, "
        f"\"bbox\".ymax - \"bbox\".ymin) > {threshold!r} "
        "THEN 'L' ELSE 'q' END"
    )


def _source_columns(
    connection: duckdb.DuckDBPyConnection, source: str | Path | Sequence[str | Path]
) -> tuple[list[str], dict[str, str]]:
    rows = connection.execute(
        f"DESCRIBE SELECT * FROM {_read_parquet(source)}"
    ).fetchall()
    columns = [row[0] for row in rows]
    types = {row[0]: row[1] for row in rows}
    required = {"id", "geometry", "bbox"}
    missing = sorted(required - set(columns))
    if missing:
        raise ValueError(f"source is missing required columns: {', '.join(missing)}")
    collisions = sorted({"leaf", "morton"} & set(columns))
    if collisions:
        raise ValueError(f"source uses reserved columns: {', '.join(collisions)}")
    return columns, types


def _validate_source(
    connection: duckdb.DuckDBPyConnection, source: str | Path | Sequence[str | Path]
) -> None:
    cx, cy = _centre_sql()
    west, south, east, north = quadtree.ROOT
    invalid = connection.execute(
        f"""
        SELECT CAST("id" AS VARCHAR)
        FROM {_read_parquet(source)}
        WHERE "id" IS NULL OR "geometry" IS NULL OR "bbox" IS NULL
           OR "bbox".xmin IS NULL OR "bbox".ymin IS NULL
           OR "bbox".xmax IS NULL OR "bbox".ymax IS NULL
           OR NOT isfinite("bbox".xmin) OR NOT isfinite("bbox".ymin)
           OR NOT isfinite("bbox".xmax) OR NOT isfinite("bbox".ymax)
           OR "bbox".xmin > "bbox".xmax OR "bbox".ymin > "bbox".ymax
           OR {cx} < {west} OR {cx} > {east}
           OR {cy} < {south} OR {cy} > {north}
        ORDER BY CAST("id" AS VARCHAR) NULLS FIRST
        LIMIT 1
        """
    ).fetchone()
    if invalid:
        raise ValueError(f"feature {invalid[0]!r} has invalid data or a centre outside ROOT")
    count = connection.execute(
        f"SELECT count(*) FROM {_read_parquet(source)}"
    ).fetchone()[0]
    if count == 0:
        raise ValueError("source contains no features")


def _histograms(
    connection: duckdb.DuckDBPyConnection,
    source: str | Path | Sequence[str | Path],
    config: DatasetConfig,
    *,
    geometry_is_wkb: bool,
) -> tuple[dict[str, dict[str, tuple[int, int]]], list[tuple[str, int, int]]]:
    cx, cy = _centre_sql()
    ix = _index_sql(cx, quadtree.ROOT[0])
    iy = _index_sql(cy, quadtree.ROOT[1])
    group = _group_sql(config.oversize_extent_deg)
    geometry_wkb = '"geometry"' if geometry_is_wkb else 'ST_AsWKB("geometry")'
    weight = (
        f"CAST(round(octet_length({geometry_wkb}) * "
        f"{config.weight_geom_ratio!r} + {config.weight_attr_bytes}) AS BIGINT)"
    )
    rows = connection.execute(
        f"""
        SELECT {group} AS feature_group, {ix} AS ix, {iy} AS iy,
               count(*)::BIGINT AS rows, sum({weight})::BIGINT AS bytes
        FROM {_read_parquet(source)}
        GROUP BY feature_group, ix, iy
        ORDER BY feature_group, ix, iy
        """
    ).fetchall()
    histograms: dict[str, dict[str, tuple[int, int]]] = {"q": {}, "L": {}}
    cells: list[tuple[str, int, int]] = []
    for group_name, x_index, y_index, count, byte_count in rows:
        fine_id = _fine_path(x_index, y_index)
        histograms[group_name][fine_id] = (count, byte_count)
        cells.append((group_name, x_index, y_index))
    return histograms, cells


def _plan(
    histograms: Mapping[str, Mapping[str, tuple[int, int]]], target_bytes: int
) -> tuple[list[quadtree.Leaf], dict[str, int]]:
    leaves: list[quadtree.Leaf] = []
    counts: dict[str, int] = {}
    for prefix in ("q", "L"):
        group_leaves = quadtree.plan_leaves(
            histograms[prefix], target_bytes=target_bytes, prefix=prefix
        )
        problems = quadtree.validate_plan(
            group_leaves, histograms[prefix], target_bytes=target_bytes
        )
        if problems:
            raise ValueError(f"invalid {prefix} partition plan: {'; '.join(problems)}")
        leaves.extend(group_leaves)
        counts[prefix] = len(group_leaves)
    return sorted(leaves, key=lambda leaf: leaf.cell_id), counts


def _owner_for_path(leaves: Sequence[quadtree.Leaf], fine_path: str) -> str:
    by_id = {leaf.cell_id for leaf in leaves}
    prefix = leaves[0].cell_id[0]
    for path_depth in range(quadtree.MAX_DEPTH + 1):
        candidate = prefix + fine_path[:path_depth]
        if candidate in by_id:
            return candidate
    raise ValueError(f"fine cell {fine_path!r} has no planned owner")


def _create_partition_table(
    connection: duckdb.DuckDBPyConnection,
    source: str | Path | Sequence[str | Path],
    columns: Sequence[str],
    cells: Sequence[tuple[str, int, int]],
    leaves: Sequence[quadtree.Leaf],
    oversize_extent_deg: float | None,
    *,
    geometry_is_wkb: bool,
) -> None:
    by_group = {
        prefix: [leaf for leaf in leaves if leaf.cell_id.startswith(prefix)]
        for prefix in ("q", "L")
    }
    ownership = []
    for group_name, ix, iy in cells:
        fine_path = _fine_path(ix, iy)[1:]
        ownership.append(
            (group_name, ix, iy, _owner_for_path(by_group[group_name], fine_path), _morton(ix, iy))
        )

    connection.execute("DROP TABLE IF EXISTS ownership")
    connection.execute("DROP TABLE IF EXISTS t")
    connection.execute(
        "CREATE TEMP TABLE ownership(feature_group VARCHAR, ix INTEGER, iy INTEGER, leaf VARCHAR, morton UBIGINT)"
    )
    connection.executemany("INSERT INTO ownership VALUES (?, ?, ?, ?, ?)", ownership)

    cx, cy = _centre_sql()
    ix_sql = _index_sql(cx, quadtree.ROOT[0])
    iy_sql = _index_sql(cy, quadtree.ROOT[1])
    group_sql = _group_sql(oversize_extent_deg)
    selected = ", ".join(
        (
            f"ST_GeomFromWKB(s.{_identifier(column)}) AS {_identifier(column)}"
            if column == "geometry" and geometry_is_wkb
            else f"s.{_identifier(column)}"
        )
        for column in columns
    )
    connection.execute(
        f"""
        CREATE TEMP TABLE t AS
        WITH s AS (
          SELECT *, {group_sql} AS __feature_group,
                 {ix_sql} AS __ix, {iy_sql} AS __iy
          FROM {_read_parquet(source)}
        )
        SELECT o.leaf, o.morton, {selected}
        FROM s
        JOIN ownership o
          ON o.feature_group = s.__feature_group
         AND o.ix = s.__ix AND o.iy = s.__iy
        ORDER BY o.leaf, o.morton, s."id"
        """
    )


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _write_shards(
    connection: duckdb.DuckDBPyConnection,
    dataset_dir: Path,
    config: DatasetConfig,
    columns: Sequence[str],
    leaves: Sequence[quadtree.Leaf],
) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    selected = ", ".join(_identifier(column) for column in columns)
    partitions: list[dict[str, Any]] = []
    estimates: list[dict[str, Any]] = []
    for leaf in leaves:
        output = dataset_dir / f"{leaf.cell_id}.parquet"
        temporary = dataset_dir / f".{leaf.cell_id}.parquet.tmp"
        if temporary.exists():
            temporary.unlink()
        connection.execute(
            f"""
            COPY (
              SELECT {selected} FROM t
              WHERE leaf = {_sql_string(leaf.cell_id)}
              ORDER BY morton, "id"
            ) TO {_sql_string(temporary)}
            (FORMAT parquet, COMPRESSION zstd, ROW_GROUP_SIZE {config.row_group_size})
            """
        )
        verify_geoparquet(temporary)
        os.replace(temporary, output)
        byte_count = output.stat().st_size
        bbox = connection.execute(
            """
            SELECT min("bbox".xmin), min("bbox".ymin),
                   max("bbox".xmax), max("bbox".ymax), count(*)
            FROM t WHERE leaf = ?
            """,
            [leaf.cell_id],
        ).fetchone()
        extent = [float(bbox[0]), float(bbox[1]), float(bbox[2]), float(bbox[3])]
        area = (extent[2] - extent[0]) * (extent[3] - extent[1])
        partition: dict[str, Any] = {
            "id": leaf.cell_id,
            "rows": bbox[4],
            "bytes": byte_count,
            "sha256": _sha256(output),
            "bbox": extent,
            "format": "GeoParquet",
            "crs": config.crs,
            "license": config.license,
            "source_release": config.source_release,
            "density": bbox[4] / area if area > 0 else None,
            "url": f"{config.dataset_id}/{leaf.cell_id}.parquet",
            "kind": "regular" if leaf.cell_id.startswith("q") else "oversize",
        }
        if config.geometry_type is not None:
            partition["geometry_type"] = config.geometry_type
        partitions.append(partition)
        estimates.append(
            {
                "id": leaf.cell_id,
                "estimated_bytes": leaf.bytes,
                "actual_bytes": byte_count,
                "estimate_error": byte_count / leaf.bytes if leaf.bytes else None,
            }
        )
    return partitions, estimates


def _union_bbox(partitions: Sequence[Mapping[str, Any]]) -> list[float]:
    return [
        min(partition["bbox"][0] for partition in partitions),
        min(partition["bbox"][1] for partition in partitions),
        max(partition["bbox"][2] for partition in partitions),
        max(partition["bbox"][3] for partition in partitions),
    ]


def _json_write(path: Path, value: Mapping[str, Any]) -> None:
    temporary = path.with_name(f".{path.name}.tmp")
    temporary.write_text(
        json.dumps(value, indent=2, sort_keys=True, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )
    os.replace(temporary, path)


def _remove_staging_directory(path: Path) -> None:
    """Remove a builder-owned staging tree without recursive delete APIs."""

    if not path.exists():
        return
    for child in sorted(path.rglob("*"), key=lambda item: len(item.parts), reverse=True):
        if child.is_file() or child.is_symlink():
            child.unlink()
        else:
            child.rmdir()
    path.rmdir()


def _remove_published_directory(path: Path) -> None:
    """Remove only files owned by this builder, then the empty directory."""

    unexpected = [
        child.name
        for child in path.iterdir()
        if not (
            child.is_file()
            and (child.suffix == ".parquet" or child.name in {"partitions.json", "build-report.json"})
        )
    ]
    if unexpected:
        raise ValueError(
            f"refusing to overwrite {path}: directory contains files not owned by the builder: "
            + ", ".join(sorted(unexpected))
        )
    for child in sorted(path.iterdir(), key=lambda item: item.name):
        child.unlink()
    path.rmdir()


def validate_build(
    source: str | Path | Sequence[str | Path],
    dataset_dir: Path,
    manifest: Mapping[str, Any],
    config: DatasetConfig,
    *,
    connection: duckdb.DuckDBPyConnection | None = None,
) -> list[str]:
    """Return all detectable conservation, ownership, metadata, and cap problems."""

    problems: list[str] = []
    owned_connection = connection is None
    if connection is None:
        connection = duckdb.connect()
        _load_spatial(connection)
    try:
        partitions = list(manifest.get("partitions", []))
        paths = [dataset_dir / f"{partition.get('id', '')}.parquet" for partition in partitions]
        existing_paths = [path for path in paths if path.is_file()]
        for path in paths:
            if not path.is_file():
                problems.append(f"missing shard file {path.name}")
        if not existing_paths:
            problems.append("build has no readable shard files")
            return problems

        source_relation = _read_parquet(source)
        shard_relation = _read_parquet(existing_paths)
        source_rows, source_distinct = connection.execute(
            f'SELECT count(*), count(DISTINCT "id") FROM {source_relation}'
        ).fetchone()
        shard_rows, shard_distinct = connection.execute(
            f'SELECT count(*), count(DISTINCT "id") FROM {shard_relation}'
        ).fetchone()
        if not (source_rows == source_distinct == shard_rows == shard_distinct):
            problems.append(
                "feature id conservation failed: "
                f"source rows/distinct={source_rows}/{source_distinct}, "
                f"shard rows/distinct={shard_rows}/{shard_distinct}"
            )
        missing = connection.execute(
            f'SELECT count(*) FROM (SELECT "id" FROM {source_relation} EXCEPT SELECT "id" FROM {shard_relation})'
        ).fetchone()[0]
        extra = connection.execute(
            f'SELECT count(*) FROM (SELECT "id" FROM {shard_relation} EXCEPT SELECT "id" FROM {source_relation})'
        ).fetchone()[0]
        if missing or extra:
            problems.append(f"feature id sets differ: {missing} missing, {extra} extra")

        manifest_rows = sum(partition.get("rows", 0) for partition in partitions)
        if manifest_rows != shard_rows or manifest.get("rows") != shard_rows:
            problems.append(
                f"manifest row totals disagree: partitions={manifest_rows}, "
                f"manifest={manifest.get('rows')}, shards={shard_rows}"
            )

        for partition, path in zip(partitions, paths):
            if not path.is_file():
                continue
            leaf_id = partition.get("id", "")
            try:
                west, south, east, north = quadtree.cell_bbox(leaf_id)
            except (TypeError, ValueError) as error:
                problems.append(f"invalid shard id {leaf_id!r}: {error}")
                continue
            relation = _read_parquet(path)
            row_count = connection.execute(f"SELECT count(*) FROM {relation}").fetchone()[0]
            if row_count == 0:
                problems.append(f"empty shard {leaf_id}")
            cx = '(("bbox".xmin + "bbox".xmax) / 2.0)'
            cy = '(("bbox".ymin + "bbox".ymax) / 2.0)'
            east_test = f"{cx} > {east}" if east == quadtree.ROOT[2] else f"{cx} >= {east}"
            north_test = f"{cy} > {north}" if north == quadtree.ROOT[3] else f"{cy} >= {north}"
            outside = connection.execute(
                f"SELECT count(*) FROM {relation} WHERE {cx} < {west} OR {east_test} OR {cy} < {south} OR {north_test}"
            ).fetchone()[0]
            if outside:
                problems.append(f"shard {leaf_id} has {outside} centres outside its cell")
            bbox = partition.get("bbox")
            if not isinstance(bbox, list) or len(bbox) != 4:
                problems.append(f"shard {leaf_id} has an invalid manifest bbox")
            else:
                escaped = connection.execute(
                    f"""
                    SELECT count(*) FROM {relation}
                    WHERE "bbox".xmin < ? OR "bbox".ymin < ?
                       OR "bbox".xmax > ? OR "bbox".ymax > ?
                    """,
                    bbox,
                ).fetchone()[0]
                if escaped:
                    problems.append(f"shard {leaf_id} bbox excludes {escaped} features")
            actual_bytes = path.stat().st_size
            if partition.get("bytes") != actual_bytes:
                problems.append(f"shard {leaf_id} byte count does not match its file")
            if partition.get("sha256") != _sha256(path):
                problems.append(f"shard {leaf_id} sha256 does not match its file")
            if actual_bytes > config.hard_cap_bytes:
                problems.append(
                    f"shard {leaf_id} is {actual_bytes} bytes, above hard cap {config.hard_cap_bytes}"
                )
    finally:
        if owned_connection:
            connection.close()
    return problems


def build_partitions(
    source: str | Path | Sequence[str | Path],
    out: Path,
    config: DatasetConfig,
    data_version: str,
    *,
    overwrite: bool = False,
    memory_limit: str = "6GB",
    temp_dir: Path | None = None,
    threads: int = 1,
) -> dict[str, Any]:
    """Build, validate, and publish one dataset's shards and two JSON records."""

    if not data_version or "latest" in data_version.casefold():
        raise ValueError("data_version must be pinned, never 'latest'")
    started = time.monotonic()
    dataset_dir = out / config.dataset_id
    staging_dir = out / f"{config.dataset_id}.building"
    if dataset_dir.exists() and not overwrite:
        raise FileExistsError(
            f"published dataset {dataset_dir} already exists; assets are immutable "
            "(pass overwrite=True or --overwrite to replace it)"
        )
    out.mkdir(parents=True, exist_ok=True)
    if staging_dir.exists():
        raise FileExistsError(f"staging directory already exists: {staging_dir}")
    staging_dir.mkdir()
    default_temp_dir = temp_dir is None
    duckdb_temp_dir = staging_dir / "duckdb-temp" if temp_dir is None else Path(temp_dir)
    connection: duckdb.DuckDBPyConnection | None = None
    try:
        duckdb_temp_dir.mkdir(parents=True, exist_ok=True)
        connection = duckdb.connect()
        _load_spatial(
            connection,
            memory_limit=memory_limit,
            temp_dir=duckdb_temp_dir,
            threads=threads,
        )
        columns, types = _source_columns(connection, source)
        geometry_type = types["geometry"].upper()
        geometry_is_wkb = geometry_type == "BLOB"
        if not geometry_is_wkb and not geometry_type.startswith("GEOMETRY"):
            raise ValueError(
                "source geometry must have DuckDB type GEOMETRY or BLOB containing WKB"
            )
        _validate_source(connection, source)
        histograms, cells = _histograms(
            connection, source, config, geometry_is_wkb=geometry_is_wkb
        )

        target = config.target_bytes
        leaves, group_counts = _plan(histograms, target)
        _create_partition_table(
            connection,
            source,
            columns,
            cells,
            leaves,
            config.oversize_extent_deg,
            geometry_is_wkb=geometry_is_wkb,
        )
        partitions, estimates = _write_shards(
            connection, staging_dir, config, columns, leaves
        )
        actual_max = max(partition["bytes"] for partition in partitions)
        if actual_max > config.hard_cap_bytes:
            first_pass_ids = {partition["id"] for partition in partitions}
            target = adjust_target(target, actual_max, config.hard_cap_bytes)
            leaves, group_counts = _plan(histograms, target)
            _create_partition_table(
                connection,
                source,
                columns,
                cells,
                leaves,
                config.oversize_extent_deg,
                geometry_is_wkb=geometry_is_wkb,
            )
            partitions, estimates = _write_shards(
                connection, staging_dir, config, columns, leaves
            )
            final_ids = {partition["id"] for partition in partitions}
            for obsolete_id in sorted(first_pass_ids - final_ids):
                (staging_dir / f"{obsolete_id}.parquet").unlink(missing_ok=True)
            offenders = [
                f"{partition['id']}={partition['bytes']}"
                for partition in partitions
                if partition["bytes"] > config.hard_cap_bytes
            ]
            if offenders:
                raise ValueError(
                    f"shards still exceed hard cap {config.hard_cap_bytes} after one replan: "
                    + ", ".join(offenders)
                )

        manifest: dict[str, Any] = {
            "schema": 3,
            "dataset": config.dataset_id,
            "data_version": data_version,
            "partition_scheme": quadtree.SCHEME,
            "min_zoom": config.min_zoom,
            "rows": sum(partition["rows"] for partition in partitions),
            "bbox": _union_bbox(partitions),
            "partitions": partitions,
        }
        problems = validate_build(
            source, staging_dir, manifest, config, connection=connection
        )
        if problems:
            raise ValueError("build validation failed: " + "; ".join(problems))

        sizes = sorted(partition["bytes"] for partition in partitions)
        report = {
            "scheme": quadtree.SCHEME,
            "config": asdict(config),
            "groups": {
                "regular": {"leaves": group_counts["q"]},
                "oversize": {"leaves": group_counts["L"]},
            },
            "planning_target_bytes": target,
            "shard_size": {
                "min": min(sizes),
                "median": statistics.median(sizes),
                "p90": sizes[max(0, math.ceil(len(sizes) * 0.9) - 1)],
                "max": max(sizes),
            },
            "shards": estimates,
            "wall_seconds": time.monotonic() - started,
        }
        _json_write(staging_dir / "partitions.json", manifest)
        _json_write(staging_dir / "build-report.json", report)
        connection.close()
        connection = None
        if default_temp_dir:
            _remove_staging_directory(duckdb_temp_dir)
        if dataset_dir.exists():
            if not overwrite:
                raise FileExistsError(
                    f"published dataset {dataset_dir} appeared while building; refusing to replace it"
                )
            if not dataset_dir.is_dir():
                raise ValueError(f"refusing to overwrite non-directory path {dataset_dir}")
            _remove_published_directory(dataset_dir)
        os.replace(staging_dir, dataset_dir)
        return manifest
    except BaseException:
        if connection is not None:
            connection.close()
        _remove_staging_directory(staging_dir)
        raise


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", required=True, help="Parquet path or glob")
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--dataset-id", required=True)
    parser.add_argument("--theme", required=True)
    parser.add_argument("--data-version", required=True)
    parser.add_argument("--source-release", required=True)
    parser.add_argument("--license", required=True)
    parser.add_argument("--target-bytes", type=int, default=25_000_000)
    parser.add_argument("--hard-cap-bytes", type=int, default=45_000_000)
    parser.add_argument("--oversize-extent-deg", type=float)
    parser.add_argument("--min-zoom", type=int, default=0)
    parser.add_argument("--geometry-type")
    parser.add_argument("--row-group-size", type=int, default=25_000)
    parser.add_argument("--memory-limit", default="6GB")
    parser.add_argument("--temp-dir", type=Path)
    parser.add_argument("--threads", type=int, default=1)
    parser.add_argument("--overwrite", action="store_true")
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    config = DatasetConfig(
        dataset_id=args.dataset_id,
        theme=args.theme,
        source_release=args.source_release,
        license=args.license,
        target_bytes=args.target_bytes,
        hard_cap_bytes=args.hard_cap_bytes,
        oversize_extent_deg=args.oversize_extent_deg,
        min_zoom=args.min_zoom,
        geometry_type=args.geometry_type,
        row_group_size=args.row_group_size,
    )
    build_partitions(
        args.source,
        args.out,
        config,
        args.data_version,
        overwrite=args.overwrite,
        memory_limit=args.memory_limit,
        temp_dir=args.temp_dir,
        threads=args.threads,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
