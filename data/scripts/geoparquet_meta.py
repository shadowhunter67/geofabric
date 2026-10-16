"""Read GeoParquet metadata without silently assigning a CRS."""

from __future__ import annotations

import json
import struct
from pathlib import Path
from typing import Any

import pyarrow.parquet as pq

_GEOMETRY_NAMES = {
    1: "Point",
    2: "LineString",
    3: "Polygon",
    4: "MultiPoint",
    5: "MultiLineString",
    6: "MultiPolygon",
    7: "GeometryCollection",
}


def geo_metadata(path: Path) -> tuple[dict[str, Any] | None, str | None]:
    """Read the GeoParquet declaration because CRS meaning belongs to file metadata."""

    metadata = pq.ParquetFile(path).metadata.metadata or {}
    raw_geo = metadata.get(b"geo")
    if raw_geo is None:
        return None, None

    geo = json.loads(raw_geo)
    primary_column = geo.get("primary_column")
    return geo, primary_column


def resolved_crs(path: Path) -> str:
    """Resolve CRS exactly as GeoParquet specifies, never assuming EPSG:4326."""

    geo, primary_column = geo_metadata(path)
    if geo is None or not primary_column:
        return "NOGEO"

    columns = geo.get("columns")
    if not isinstance(columns, dict) or primary_column not in columns:
        return "NOGEO"

    column = columns[primary_column]
    if not isinstance(column, dict):
        return "NOGEO"

    if "crs" not in column:
        return "OGC:CRS84"

    crs = column["crs"]
    if crs is None:
        return "UNDEFINED"

    if not isinstance(crs, dict):
        return "PROJJSON:"

    identifier = crs.get("id")
    if isinstance(identifier, dict):
        authority = identifier.get("authority")
        code = identifier.get("code")
        if authority is not None and code is not None:
            return f"{authority}:{code}"

    return f"PROJJSON:{crs.get('name', '')}"


def _type_and_dimensions(raw_type: int) -> tuple[int, int]:
    """Decode ISO and EWKB type flags so WKB bbox parsing covers Z/M variants."""

    has_z = bool(raw_type & 0x80000000)
    has_m = bool(raw_type & 0x40000000)
    geometry_type = raw_type & 0x1FFFFFFF

    if geometry_type >= 3000:
        return geometry_type - 3000, 4
    if geometry_type >= 2000:
        return geometry_type - 2000, 3
    if geometry_type >= 1000:
        return geometry_type - 1000, 3

    return geometry_type, 2 + int(has_z) + int(has_m)


def _read_uint32(data: memoryview, offset: int, endian: str) -> tuple[int, int]:
    """Advance safely through WKB counts so malformed geometry stays detectable."""

    return struct.unpack_from(f"{endian}I", data, offset)[0], offset + 4


def _read_point(
    data: memoryview,
    offset: int,
    endian: str,
    dimensions: int,
) -> tuple[tuple[float, float], int]:
    """Read XY while skipping Z/M ordinates because a bbox is two-dimensional."""

    values = struct.unpack_from(f"{endian}{'d' * dimensions}", data, offset)
    return (values[0], values[1]), offset + (8 * dimensions)


def _read_geometry(
    data: memoryview,
    offset: int = 0,
) -> tuple[str, bool, list[tuple[float, float]], int]:
    """Recursively collect XY coordinates for all standard WKB geometry families."""

    byte_order = data[offset]
    if byte_order not in (0, 1):
        raise ValueError("invalid byte order")
    endian = "<" if byte_order == 1 else ">"
    offset += 1

    raw_type, offset = _read_uint32(data, offset, endian)
    geometry_type, dimensions = _type_and_dimensions(raw_type)
    name = _GEOMETRY_NAMES.get(geometry_type, "Unknown")
    if geometry_type not in _GEOMETRY_NAMES:
        raise ValueError("unknown geometry type")

    if geometry_type == 1:
        point, offset = _read_point(data, offset, endian, dimensions)
        return name, False, [point], offset

    if geometry_type == 2:
        count, offset = _read_uint32(data, offset, endian)
        points: list[tuple[float, float]] = []
        for _ in range(count):
            point, offset = _read_point(data, offset, endian, dimensions)
            points.append(point)
        return name, count == 0, points, offset

    if geometry_type == 3:
        ring_count, offset = _read_uint32(data, offset, endian)
        points = []
        for _ in range(ring_count):
            point_count, offset = _read_uint32(data, offset, endian)
            for _ in range(point_count):
                point, offset = _read_point(data, offset, endian, dimensions)
                points.append(point)
        return name, ring_count == 0, points, offset

    geometry_count, offset = _read_uint32(data, offset, endian)
    points = []
    for _ in range(geometry_count):
        _, _, child_points, offset = _read_geometry(data, offset)
        points.extend(child_points)

    return name, geometry_count == 0, points, offset


def wkb_info(value: bytes | bytearray | memoryview | None) -> tuple[str | None, bool, list[float] | None]:
    """Return geometry facts from WKB when no trustworthy stored bbox is available."""

    if value is None:
        return None, False, None

    try:
        name, empty, points, _ = _read_geometry(memoryview(value))
    except (IndexError, ValueError, struct.error):
        return "InvalidWKB", False, None

    if not points:
        return name, empty, None

    xs = [point[0] for point in points]
    ys = [point[1] for point in points]
    return name, empty, [min(xs), min(ys), max(xs), max(ys)]
