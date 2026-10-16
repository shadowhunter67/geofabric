"""Load and validate dataset manifests to preserve reproducible data lineage."""

from __future__ import annotations

import hashlib
import json
import re
from datetime import date
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]

GROUPS = {
    "base",
    "buildings",
    "transport",
    "admin",
    "places",
    "raster",
    "thematic",
    "benchmark",
}
SOURCE_TYPES = {"overture-s3", "http", "manual", "generated"}
REDISTRIBUTION_VALUES = {
    "allowed",
    "with-attribution",
    "share-alike",
    "restricted",
    "unclear",
}
NORMALIZED_FORMATS = {"GeoParquet", "COG", "GeoJSON"}

TOP_LEVEL_FIELDS = {
    "id",
    "title",
    "group",
    "provider",
    "source",
    "license",
    "retrieved",
    "coverage",
    "crs",
    "format_original",
    "format_normalized",
    "geometry_type",
    "files",
    "fetch",
    "notes",
}


def fail(manifest_id: str, field: str, message: str) -> None:
    """Raise a field-specific error so invalid lineage is actionable."""

    raise ValueError(f"{manifest_id}: {field}: {message}")


def sha256_file(path: Path) -> str:
    """Hash incrementally so verification remains safe for multi-GB files."""

    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def digest_ids(values: list[Any]) -> str:
    """Digest sorted UTF-8 id strings joined by newline for small test inputs."""

    encoded_values = sorted(str(value) for value in values)
    return hashlib.sha256("\n".join(encoded_values).encode("utf-8")).hexdigest()


def validate(manifest: Any, filename_id: str | None = None) -> None:
    """Validate explicit dataset provenance before any data operation uses it."""

    manifest_id = filename_id or "<unknown>"
    if isinstance(manifest, dict) and isinstance(manifest.get("id"), str):
        manifest_id = manifest["id"]

    if not isinstance(manifest, dict):
        fail(manifest_id, "manifest", "must be an object")

    unknown_fields = sorted(set(manifest) - TOP_LEVEL_FIELDS)
    if unknown_fields:
        fail(manifest_id, unknown_fields[0], "unknown top-level field")

    for field in sorted(TOP_LEVEL_FIELDS - {"notes"}):
        if field not in manifest:
            fail(manifest_id, field, "is required")

    dataset_id = manifest["id"]
    if not isinstance(dataset_id, str) or not re.fullmatch(r"[a-z][a-z0-9_]*", dataset_id):
        fail(manifest_id, "id", "must be snake_case")
    if filename_id is not None and dataset_id != filename_id:
        fail(dataset_id, "id", "must equal filename")

    title = manifest["title"]
    if (
        not isinstance(title, dict)
        or not isinstance(title.get("vi"), str)
        or not title["vi"]
        or not isinstance(title.get("en"), str)
        or not title["en"]
    ):
        fail(dataset_id, "title", "requires non-empty vi and en strings")

    if manifest["group"] not in GROUPS:
        fail(dataset_id, "group", "must be a supported group")

    if not isinstance(manifest["provider"], str) or not manifest["provider"]:
        fail(dataset_id, "provider", "must be non-empty")

    source = manifest["source"]
    if not isinstance(source, dict):
        fail(dataset_id, "source", "must be an object")
    if source.get("type") not in SOURCE_TYPES:
        fail(dataset_id, "source.type", "must be a supported source type")
    if not isinstance(source.get("reference"), str) or not source["reference"]:
        fail(dataset_id, "source.reference", "must be non-empty")
    release = source.get("release")
    if not isinstance(release, str) or release.strip().lower() in {"", "latest", "current"}:
        fail(dataset_id, "source.release", "must be a pinned release or date")

    license_info = manifest["license"]
    if not isinstance(license_info, dict):
        fail(dataset_id, "license", "must be an object")
    for field in ("name", "url", "attribution"):
        if not isinstance(license_info.get(field), str) or not license_info[field]:
            fail(dataset_id, f"license.{field}", "must be non-empty")
    redistribution = license_info.get("redistribution")
    if redistribution not in REDISTRIBUTION_VALUES:
        fail(dataset_id, "license.redistribution", "must be supported")
    if redistribution == "unclear":
        fail(dataset_id, "license.redistribution", "unclear licenses are not accepted")

    try:
        date.fromisoformat(manifest["retrieved"])
    except (TypeError, ValueError):
        fail(dataset_id, "retrieved", "must be an ISO date")

    coverage = manifest["coverage"]
    if not isinstance(coverage, dict):
        fail(dataset_id, "coverage", "must be an object")
    if not isinstance(coverage.get("name"), str) or not coverage["name"]:
        fail(dataset_id, "coverage.name", "must be non-empty")

    bbox = coverage.get("bbox")
    valid_bbox = (
        isinstance(bbox, list)
        and len(bbox) == 4
        and all(isinstance(value, (int, float)) and not isinstance(value, bool) for value in bbox)
        and -180 <= bbox[0] < bbox[2] <= 180
        and -90 <= bbox[1] < bbox[3] <= 90
    )
    if not valid_bbox:
        fail(dataset_id, "coverage.bbox", "must be an ordered longitude/latitude bbox")

    if not isinstance(manifest["crs"], str) or not manifest["crs"]:
        fail(dataset_id, "crs", "must be explicit and non-empty")

    if not isinstance(manifest["format_original"], str) or not manifest["format_original"]:
        fail(dataset_id, "format_original", "must be non-empty")

    if manifest["format_normalized"] not in NORMALIZED_FORMATS:
        fail(dataset_id, "format_normalized", "must be supported")

    if not isinstance(manifest["geometry_type"], str) or not manifest["geometry_type"]:
        fail(dataset_id, "geometry_type", "must be non-empty")

    files = manifest["files"]
    if not isinstance(files, list):
        fail(dataset_id, "files", "must be a list")

    for index, file_info in enumerate(files):
        field = f"files[{index}]"
        if not isinstance(file_info, dict):
            fail(dataset_id, field, "must be an object")

        path = file_info.get("path")
        if (
            not isinstance(path, str)
            or not path
            or "\\" in path
            or Path(path).is_absolute()
            or path.startswith("../")
        ):
            fail(dataset_id, f"{field}.path", "must be repository-relative with forward slashes")

        size_bytes = file_info.get("size_bytes")
        if not isinstance(size_bytes, int) or isinstance(size_bytes, bool) or size_bytes < 0:
            fail(dataset_id, f"{field}.size_bytes", "must be a non-negative integer")

        sha256 = file_info.get("sha256")
        if not isinstance(sha256, str) or not re.fullmatch(r"[0-9a-fA-F]{64}", sha256):
            fail(dataset_id, f"{field}.sha256", "must be 64 hexadecimal characters")

        rows = file_info.get("rows")
        if rows is not None and (
            not isinstance(rows, int) or isinstance(rows, bool) or rows < 0
        ):
            fail(dataset_id, f"{field}.rows", "must be a non-negative integer or null")

        id_digest = file_info.get("id_digest")
        if id_digest is not None and (
            not isinstance(id_digest, str) or not re.fullmatch(r"[0-9a-fA-F]{64}", id_digest)
        ):
            fail(dataset_id, f"{field}.id_digest", "must be 64 hexadecimal characters or null")

    fetch = manifest["fetch"]
    if not isinstance(fetch, dict):
        fail(dataset_id, "fetch", "must be an object")
    if not isinstance(fetch.get("script"), str) or not fetch["script"]:
        fail(dataset_id, "fetch.script", "must be non-empty")
    if not isinstance(fetch.get("args"), list) or not all(
        isinstance(argument, str) for argument in fetch["args"]
    ):
        fail(dataset_id, "fetch.args", "must be a list of strings")

    if "notes" in manifest and not isinstance(manifest["notes"], str):
        fail(dataset_id, "notes", "must be a string")


def load(path: Path) -> dict[str, Any]:
    """Load and validate immediately so callers never receive unchecked metadata."""

    manifest = json.loads(path.read_text(encoding="utf-8"))
    validate(manifest, path.stem)
    return manifest


def dump(path: Path, manifest: dict[str, Any]) -> None:
    """Write deterministic UTF-8 JSON for reviewable manifest changes."""

    path.write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
        newline="\n",
    )
